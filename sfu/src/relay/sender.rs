use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
    sync::broadcast,
};
use webrtc::{rtp, rtp_transceiver::rtp_codec::RTCRtpCodecParameters};

use crate::{
    error::{self, Error},
    publisher::PublisherType,
    relay::{
        data::{PacketData, TrackData},
        receiver::TCPResponse,
    },
    rtp::layer::Layer,
};

use super::data::RTCRtpCodecParametersSerializable;

#[derive(Debug)]
pub(crate) struct RelaySender {
    /// UDP socket for sending RTP packets.
    socket: UdpSocket,
}

impl RelaySender {
    pub(crate) async fn new(port: u16) -> Result<Self, Error> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", port)).await?;
        Ok(Self { socket })
    }

    pub(crate) async fn create_relay_track(
        &self,
        ip: String,
        port: u16,
        router_id: String,
        track_id: String,
        ssrc: u32,
        codec_parameters: RTCRtpCodecParameters,
        stream_id: String,
        mime_type: String,
        rid: String,
        publisher_type: PublisherType,
    ) -> Result<u16, Error> {
        let addr = format!("{}:{}", ip, port);
        let mut stream = TcpStream::connect(addr).await?;

        tracing::debug!("creating relay track with {:#?}", publisher_type);
        let data = TrackData {
            router_id,
            track_id,
            ssrc,
            codec_parameters: codec_parameters.into(),
            stream_id,
            mime_type,
            rid,
            closed: false,
            publisher_type,
        };
        let d = serde_json::to_string(&data).unwrap();
        stream.write(d.as_bytes()).await?;

        let mut buf = Vec::with_capacity(4096);
        stream.read_buf(&mut buf).await?;

        match bincode::decode_from_slice(&buf, bincode::config::standard()) {
            Ok((response, _len)) => {
                let received: TCPResponse = response;
                if let Some(port) = received.port {
                    return Ok(port);
                } else {
                    return Err(Error::new_relay(
                        "UDP port not found".to_string(),
                        error::RelayErrorKind::RelaySenderError,
                    ));
                }
            }
            Err(err) => {
                tracing::error!("Failed to decode TCP response: {}", err);
                return Err(Error::from(err));
            }
        }
    }

    pub(crate) async fn remove_relayed_publisher(
        &self,
        ip: String,
        port: u16,
        router_id: String,
        track_id: String,
    ) -> Result<bool, Error> {
        let addr = format!("{}:{}", ip, port);
        let mut stream = TcpStream::connect(addr).await?;

        let data = TrackData {
            router_id,
            track_id,
            ssrc: 0,
            codec_parameters: RTCRtpCodecParametersSerializable::default(),
            stream_id: "".to_owned(),
            mime_type: "".to_owned(),
            rid: "".to_owned(),
            closed: true,
            publisher_type: PublisherType::Simple,
        };
        let d = serde_json::to_string(&data).unwrap();
        stream.write(d.as_bytes()).await?;

        let mut buf = Vec::with_capacity(4096);
        stream.read_buf(&mut buf).await?;

        let received = String::from_utf8(buf).unwrap();
        if received == "ok" {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(crate) async fn rtp_sender_loop(
        &self,
        ip: String,
        port: u16,
        ssrc: u32,
        track_id: String,
        rtp_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
    ) {
        let mut rtp_receiver = rtp_sender.subscribe();
        drop(rtp_sender);

        tracing::debug!("Relay sender publisher_ssrc={} RTP loop has started", ssrc);

        let addr = format!("{}:{}", ip, port);

        loop {
            let res = rtp_receiver.recv().await;
            match res {
                Ok((packet, layer)) => {
                    let data = PacketData {
                        packet,
                        layer,
                        ssrc,
                        track_id: track_id.clone(),
                    };
                    tracing::trace!("sending packet: {}", track_id);
                    match data.marshal() {
                        Ok(send_buf) => {
                            if let Err(err) = self.socket.send_to(&send_buf, addr.clone()).await {
                                tracing::error!("Failed to send packet with UDP: {}", err);
                            }
                        }
                        Err(err) => {
                            tracing::error!("Failed to marshal data: {}", err);
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
                Err(err) => {
                    tracing::error!(
                        "Relay sender publisher_ssrc={} failed to read rtp: {}",
                        ssrc,
                        err
                    );
                }
            }
        }

        tracing::debug!("Relay sender publisher_ssrc={} RTP loop has finished", ssrc);
    }
}

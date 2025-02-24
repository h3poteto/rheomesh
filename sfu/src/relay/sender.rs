use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
    sync::broadcast,
};
use webrtc::{rtp, rtp_transceiver::rtp_codec::RTCRtpCodecCapability};

use crate::{
    error::Error,
    relay::data::{PacketData, TrackData},
    rtp::layer::Layer,
};

use super::data::RTCRtpCodecCapabilitySerializable;

#[derive(Debug)]
pub(crate) struct RelaySender {
    socket: UdpSocket,
    server_udp_port: u16,
    server_tcp_port: u16,
}

impl RelaySender {
    pub(crate) async fn new(
        port: u16,
        server_tcp_port: u16,
        server_udp_port: u16,
    ) -> Result<Self, Error> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", port)).await?;
        Ok(Self {
            socket,
            server_udp_port,
            server_tcp_port,
        })
    }

    pub(crate) async fn create_relay_track(
        &self,
        ip: String,
        router_id: String,
        track_id: String,
        ssrc: u32,
        codec_capability: RTCRtpCodecCapability,
        stream_id: String,
        mime_type: String,
        rid: String,
    ) -> Result<bool, Error> {
        let addr = format!("{}:{}", ip, self.server_tcp_port);
        let mut stream = TcpStream::connect(addr).await?;

        let data = TrackData {
            router_id,
            track_id,
            ssrc,
            codec_capability: codec_capability.into(),
            stream_id,
            mime_type,
            rid,
            closed: false,
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

    pub(crate) async fn remove_relayed_publisher(
        &self,
        ip: String,
        router_id: String,
        track_id: String,
    ) -> Result<bool, Error> {
        let addr = format!("{}:{}", ip, self.server_tcp_port);
        let mut stream = TcpStream::connect(addr).await?;

        let data = TrackData {
            router_id,
            track_id,
            ssrc: 0,
            codec_capability: RTCRtpCodecCapabilitySerializable::default(),
            stream_id: "".to_owned(),
            mime_type: "".to_owned(),
            rid: "".to_owned(),
            closed: true,
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
        ssrc: u32,
        track_id: String,
        rtp_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
    ) {
        let mut rtp_receiver = rtp_sender.subscribe();
        drop(rtp_sender);

        tracing::debug!("Relay sender publisher_ssrc={} RTP loop has started", ssrc);

        let addr = format!("{}:{}", ip, self.server_udp_port);

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

use std::sync::Arc;

use bytes::Bytes;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
    sync::broadcast,
};
use webrtc::{
    data_channel::data_channel_message::DataChannelMessage, rtcp, rtp,
    rtp_transceiver::rtp_codec::RTCRtpCodecParameters,
};

use crate::{
    error::{self, Error},
    publisher::PublisherType,
    relay::{
        data::{ChannelData, PacketData, RelayMessage, TrackData},
        receiver::TCPResponse,
    },
    rtp::layer::Layer,
    transport,
    utils::ports::find_unused_port,
};

use super::data::{MessageData, RTCRtpCodecParametersSerializable};

#[derive(Debug)]
pub(crate) struct RelaySender {}

impl RelaySender {
    pub(crate) async fn new() -> Self {
        Self {}
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
        sender_udp_port: u16,
        sender_ip: String,
    ) -> Result<u16, Error> {
        let addr = format!("{}:{}", ip, port);
        let mut stream = TcpStream::connect(addr).await?;

        tracing::debug!("creating relay track with {:#?}", publisher_type);
        let data = RelayMessage::Track(TrackData {
            router_id,
            track_id,
            ssrc,
            codec_parameters: codec_parameters.into(),
            stream_id,
            mime_type,
            rid,
            closed: false,
            publisher_type,
            udp_port: Some(sender_udp_port),
            ip: Some(sender_ip),
        });
        let d = serde_json::to_string(&data)?;
        stream.write(d.as_bytes()).await?;

        let mut buf = Vec::with_capacity(4096);
        stream.read_buf(&mut buf).await?;

        match bincode::decode_from_slice(&buf, bincode::config::standard()) {
            Ok((response, _len)) => {
                let received: TCPResponse = response;
                if let Some(udp_started) = received.udp_started {
                    tracing::debug!(
                        "Relay track created successfully with UDP port: {}",
                        udp_started.port
                    );
                    return Ok(udp_started.port);
                } else {
                    tracing::error!("Failed to create relay track: {}", received.status);
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

        let data = RelayMessage::Track(TrackData {
            router_id,
            track_id,
            ssrc: 0,
            codec_parameters: RTCRtpCodecParametersSerializable::default(),
            stream_id: "".to_owned(),
            mime_type: "".to_owned(),
            rid: "".to_owned(),
            closed: true,
            publisher_type: PublisherType::Simple,
            udp_port: None,
            ip: None,
        });
        let d = serde_json::to_string(&data).unwrap();
        stream.write(d.as_bytes()).await?;

        let mut buf = Vec::with_capacity(4096);
        stream.read_buf(&mut buf).await?;

        match bincode::decode_from_slice(&buf, bincode::config::standard()) {
            Ok((response, _len)) => {
                let received: TCPResponse = response;
                if received.status == "ok" {
                    return Ok(true);
                } else {
                    return Err(Error::new_relay(
                        "Failed to remove relay publisher".to_string(),
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

    pub(crate) async fn create_relay_data(
        &self,
        ip: String,
        port: u16,
        router_id: String,
        data_publisher_id: String,
        channel_id: u16,
        label: String,
    ) -> Result<u16, Error> {
        let addr = format!("{}:{}", ip, port);
        let mut stream = TcpStream::connect(addr).await?;

        tracing::debug!("creating relay data channel");
        let data = RelayMessage::Channel(ChannelData {
            router_id,
            data_publisher_id,
            channel_id,
            label,
            closed: false,
        });
        let d = serde_json::to_string(&data)?;
        stream.write(d.as_bytes()).await?;

        let mut buf = Vec::with_capacity(4096);
        stream.read_buf(&mut buf).await?;

        match bincode::decode_from_slice(&buf, bincode::config::standard()) {
            Ok((response, _len)) => {
                let received: TCPResponse = response;
                if let Some(udp_started) = received.udp_started {
                    tracing::debug!(
                        "Relay data channel created successfully with UDP port: {}",
                        udp_started.port
                    );
                    return Ok(udp_started.port);
                } else {
                    tracing::error!("Failed to create relay data channel: {}", received.status);
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
}

#[derive(Debug)]
pub(crate) struct RelayUDPSender {
    socket: UdpSocket,
    pub(crate) port: u16,
}

impl RelayUDPSender {
    pub(crate) async fn new() -> Result<Self, Error> {
        if let Some(port) = find_unused_port() {
            let socket = UdpSocket::bind(format!("0.0.0.0:{}", port)).await?;
            return Ok(Self { socket, port });
        }
        Err(Error::new_relay(
            "Failed to find unused port for UDP sender".to_string(),
            error::RelayErrorKind::RelaySenderError,
        ))
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

    pub(crate) async fn rtcp_receiver_loop(&self, rtcp_sender: Arc<transport::RtcpSender>) {
        loop {
            let mut buf = [0u8; 1500];

            let res = self.socket.recv_from(&mut buf).await;
            match res {
                Ok((len, _addr)) => {
                    let mut bytes = Bytes::copy_from_slice(&buf[..len]);
                    let data = rtcp::packet::unmarshal(&mut bytes);
                    match data {
                        Ok(rtcp) => {
                            for d in rtcp.to_vec() {
                                if let Err(err) = rtcp_sender.send(d) {
                                    tracing::error!(
                                        "Failed to send RTCP packet to publish_transport: {}",
                                        err
                                    );
                                    continue;
                                }
                            }
                        }
                        Err(err) => {
                            tracing::error!("Failed to unmarshal RTCP packet: {}", err);
                            continue;
                        }
                    }
                }
                Err(err) => {
                    tracing::error!("Failed to receive RTCP packet: {}", err);
                    continue;
                }
            }
        }
    }

    pub(crate) async fn data_sender_loop(
        &self,
        ip: String,
        port: u16,
        data_publisher_id: String,
        data_sender: broadcast::Sender<DataChannelMessage>,
    ) {
        let mut data_receiver = data_sender.subscribe();
        drop(data_sender);

        let addr = format!("{}:{}", ip, port);

        loop {
            let res = data_receiver.recv().await;
            match res {
                Ok(msg) => {
                    let data = MessageData {
                        data_publisher_id: data_publisher_id.clone(),
                        message: msg,
                    };
                    tracing::debug!("sending data message: {}", data_publisher_id);
                    match data.marshal() {
                        Ok(send_buf) => {
                            if let Err(err) = self.socket.send_to(&send_buf, addr.clone()).await {
                                tracing::error!("Failed to send data message with UDP: {}", err);
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
                    tracing::error!("Failed to receive data message: {}", err);
                    continue;
                }
            }
        }
    }
}

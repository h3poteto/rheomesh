use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use webrtc::rtp;
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters};
use webrtc::rtp_transceiver::PayloadType;

use crate::rtp::layer::Layer;
use crate::track::Track;
use crate::transport;

/// Track that is received from another server.
#[derive(Debug)]
pub struct RelayedTrack {
    /// The ID is the same as published track_id.
    id: String,
    ssrc: u32,
    rid: String,
    mime_type: String,
    codec_parameters: RTCRtpCodecParameters,
    stream_id: String,
    rtp_packet_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
    rtcp_sender: Arc<transport::RtcpSender>,
}

impl RelayedTrack {
    pub fn new(
        id: String,
        ssrc: u32,
        rid: String,
        mime_type: String,
        codec_parameters: RTCRtpCodecParameters,
        stream_id: String,
    ) -> Self {
        let (sender, _reader) = broadcast::channel::<(rtp::packet::Packet, Layer)>(1024);
        // For relayed track, RTCP will not work properly.
        // Even though it receives RTCP packets, this server doesn't send it to the source publisher.
        // So, this rtcp_sender is dummy.
        let (rtcp_sender, rtcp_receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            Self::rtcp_event_loop(rtcp_receiver).await;
        });

        Self {
            id,
            ssrc,
            rid,
            mime_type,
            codec_parameters,
            stream_id,
            rtp_packet_sender: sender,
            rtcp_sender: Arc::new(rtcp_sender),
        }
    }

    async fn rtcp_event_loop(mut rtcp_receiver: transport::RtcpReceiver) {
        loop {
            if let Some(data) = rtcp_receiver.recv().await {
                tracing::trace!("Relayed rtcp: {}", data);
            }
        }
    }
}

impl Track for RelayedTrack {
    fn rtcp_sender(&self) -> Arc<transport::RtcpSender> {
        self.rtcp_sender.clone()
    }

    fn rtp_packet_sender(&self) -> broadcast::Sender<(rtp::packet::Packet, Layer)> {
        self.rtp_packet_sender.clone()
    }

    fn mime_type(&self) -> String {
        self.mime_type.clone()
    }

    fn payload_type(&self) -> PayloadType {
        self.codec_parameters.payload_type.clone().into()
    }

    fn parameters(&self) -> RTCRtpCodecParameters {
        self.codec_parameters.clone().into()
    }

    fn capability(&self) -> RTCRtpCodecCapability {
        self.codec_parameters.capability.clone().into()
    }

    fn id(&self) -> String {
        self.id.clone()
    }

    fn stream_id(&self) -> String {
        self.stream_id.clone()
    }

    fn ssrc(&self) -> u32 {
        self.ssrc
    }

    fn rid(&self) -> String {
        self.rid.clone()
    }

    fn close(&self) {}
}

impl Drop for RelayedTrack {
    fn drop(&mut self) {
        tracing::debug!("RelaydTrack id={} ssrc={} is dropped", self.id, self.ssrc);
    }
}

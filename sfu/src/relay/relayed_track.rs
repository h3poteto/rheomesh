use tokio::sync::broadcast;
use webrtc::rtp;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;

use crate::rtp::layer::Layer;

#[derive(Debug)]
pub struct RelayedTrack {
    /// The ID is the same as published track_id.
    pub id: String,
    pub ssrc: u32,
    pub rid: String,
    pub mime_type: String,
    pub codec_capability: RTCRtpCodecCapability,
    pub stream_id: String,
    pub(crate) rtp_packet_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
}

impl RelayedTrack {
    pub fn new(
        id: String,
        ssrc: u32,
        rid: String,
        mime_type: String,
        codec_capability: RTCRtpCodecCapability,
        stream_id: String,
    ) -> Self {
        let (sender, _reader) = broadcast::channel::<(rtp::packet::Packet, Layer)>(1024);
        Self {
            id,
            ssrc,
            rid,
            mime_type,
            codec_capability,
            stream_id,
            rtp_packet_sender: sender,
        }
    }
}

impl Drop for RelayedTrack {
    fn drop(&mut self) {
        tracing::debug!("RelaydTrack id={} ssrc={} is dropped", self.id, self.ssrc);
    }
}

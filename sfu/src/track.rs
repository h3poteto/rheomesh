use std::sync::Arc;

use tokio::sync::broadcast;
use webrtc::rtp;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;

use crate::{rtp::layer::Layer, transport};

pub trait Track {
    fn rtcp_sender(&self) -> Arc<transport::RtcpSender>;
    fn rtp_packet_sender(&self) -> broadcast::Sender<(rtp::packet::Packet, Layer)>;
    fn mime_type(&self) -> String;
    fn capability(&self) -> RTCRtpCodecCapability;
    fn id(&self) -> String;
    fn stream_id(&self) -> String;
    fn ssrc(&self) -> u32;
    fn rid(&self) -> String;
    fn close(&self);
}

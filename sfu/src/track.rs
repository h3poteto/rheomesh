use std::sync::Arc;

use rtc::{
    media_stream::MediaStreamId,
    rtp,
    rtp_transceiver::{
        PayloadType,
        rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters},
    },
};
use tokio::sync::broadcast;

use crate::{rtp::layer::Layer, transport};

/// Track represent media track that can be subscribed.
pub trait Track {
    fn rtcp_sender(&self) -> Arc<transport::RtcpSender>;
    fn rtp_packet_sender(&self) -> broadcast::Sender<(rtp::packet::Packet, Layer)>;
    fn mime_type(&self) -> String;
    fn payload_type(&self) -> PayloadType;
    fn parameters(&self) -> RTCRtpCodecParameters;
    fn capability(&self) -> RTCRtpCodec;
    fn id(&self) -> String;
    fn stream_id(&self) -> MediaStreamId;
    fn ssrc(&self) -> u32;
    fn rid(&self) -> String;
    fn close(&self);
}

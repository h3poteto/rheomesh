use rtc::{
    rtcp,
    rtp_transceiver::rtp_sender::{RTCRtpHeaderExtensionCapability, RtpCodecKind},
};
use std::sync::Arc;
use tokio::sync::mpsc;
use webrtc::{
    media_stream::track_remote::TrackRemote,
    peer_connection::{
        MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCIceCandidate, RTCIceCandidateInit, RTCIceGatheringState, RTCPeerConnectionState,
        RTCSessionDescription, RTCSignalingState, RTCStatsReport, Registry,
        register_default_interceptors,
    },
};

use crate::{
    config::{MediaConfig, WebRTCTransportConfig},
    error::Error,
};

pub(crate) type RtcpSender = mpsc::UnboundedSender<Box<dyn rtcp::packet::Packet>>;
pub(crate) type RtcpReceiver = mpsc::UnboundedReceiver<Box<dyn rtcp::packet::Packet>>;

pub type OnIceCandidateFn = Box<dyn Fn(RTCIceCandidate) + Send + Sync>;
pub type OnNegotiationNeededFn = Box<dyn Fn(RTCSessionDescription) + Send + Sync>;
pub type OnTrackFn = Box<dyn Fn(Arc<dyn TrackRemote>) + Send + Sync>;

pub(crate) async fn generate_peer_connection(
    handler: Arc<dyn PeerConnectionEventHandler>,
    media_config: MediaConfig,
    transport_config: WebRTCTransportConfig,
) -> Result<Arc<dyn PeerConnection>, Error> {
    let mut me = MediaEngine::default();

    if media_config.codec.audio.len() > 0 || media_config.codec.video.len() > 0 {
        for codec in media_config.codec.audio {
            me.register_codec(codec, RtpCodecKind::Audio)?;
        }
        for codec in media_config.codec.video {
            me.register_codec(codec, RtpCodecKind::Video)?;
        }
    } else {
        me.register_default_codecs()?;
    }

    for extension in media_config.header_extension.audio {
        me.register_header_extension(
            RTCRtpHeaderExtensionCapability { uri: extension },
            RtpCodecKind::Audio,
            None,
        )?;
    }

    for extension in media_config.header_extension.video {
        me.register_header_extension(
            RTCRtpHeaderExtensionCapability { uri: extension },
            RtpCodecKind::Video,
            None,
        )?;
    }

    let registry = Registry::new();
    let registry = register_default_interceptors(registry, &mut me)?;

    let peer_connection = PeerConnectionBuilder::new()
        .with_configuration(transport_config.configuration.clone())
        .with_media_engine(me)
        .with_interceptor_registry(registry)
        .with_setting_engine(transport_config.setting_engine())
        .with_udp_addrs(transport_config.udp_addrs()?)
        .with_handler(handler)
        .build()
        .await?;

    Ok(Arc::new(peer_connection))
}

pub trait Transport {
    fn add_ice_candidate(
        &self,
        candidate: RTCIceCandidateInit,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send;

    fn get_stats(&self) -> impl std::future::Future<Output = RTCStatsReport> + Send;

    fn signaling_state(&self) -> RTCSignalingState;

    fn ice_gathering_state(&self) -> RTCIceGatheringState;

    fn connection_state(&self) -> RTCPeerConnectionState;
}

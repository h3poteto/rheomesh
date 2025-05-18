use std::sync::Arc;
use tokio::sync::mpsc;
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors, media_engine::MediaEngine, APIBuilder,
    },
    ice_transport::{
        ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
        ice_gathering_state::RTCIceGatheringState,
    },
    interceptor::registry::Registry,
    peer_connection::{
        peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription, signaling_state::RTCSignalingState,
        RTCPeerConnection,
    },
    rtcp,
    rtp_transceiver::{
        rtp_codec::{RTCRtpHeaderExtensionCapability, RTPCodecType},
        rtp_receiver::RTCRtpReceiver,
        RTCRtpTransceiver,
    },
    stats,
    track::track_remote::TrackRemote,
};

use crate::{
    config::{MediaConfig, WebRTCTransportConfig},
    error::Error,
};

pub(crate) type RtcpSender = mpsc::UnboundedSender<Box<dyn rtcp::packet::Packet + Send + Sync>>;
pub(crate) type RtcpReceiver = mpsc::UnboundedReceiver<Box<dyn rtcp::packet::Packet + Send + Sync>>;

pub type OnIceCandidateFn = Box<dyn Fn(RTCIceCandidate) + Send + Sync>;
pub type OnNegotiationNeededFn = Box<dyn Fn(RTCSessionDescription) + Send + Sync>;
pub type OnTrackFn =
    Box<dyn Fn(Arc<TrackRemote>, Arc<RTCRtpReceiver>, Arc<RTCRtpTransceiver>) + Send + Sync>;

pub(crate) trait PeerConnection {
    fn generate_peer_connection(
        media_config: MediaConfig,
        transport_config: WebRTCTransportConfig,
    ) -> impl std::future::Future<Output = Result<RTCPeerConnection, Error>> + Send {
        async move {
            let mut me = MediaEngine::default();

            if media_config.codec.audio.len() > 0 || media_config.codec.video.len() > 0 {
                for codec in media_config.codec.audio {
                    me.register_codec(codec, RTPCodecType::Audio)?;
                }
                for codec in media_config.codec.video {
                    me.register_codec(codec, RTPCodecType::Video)?;
                }
            } else {
                me.register_default_codecs()?;
            }

            for extension in media_config.header_extension.audio {
                me.register_header_extension(
                    RTCRtpHeaderExtensionCapability { uri: extension },
                    RTPCodecType::Audio,
                    None,
                )?;
            }

            for extension in media_config.header_extension.video {
                me.register_header_extension(
                    RTCRtpHeaderExtensionCapability { uri: extension },
                    RTPCodecType::Video,
                    None,
                )?;
            }

            let mut registry = Registry::new();
            registry = register_default_interceptors(registry, &mut me)?;

            let api = APIBuilder::new()
                .with_media_engine(me)
                .with_interceptor_registry(registry)
                .with_setting_engine(transport_config.setting_engine())
                .build();

            let peer_connection = api
                .new_peer_connection(transport_config.configuration.clone())
                .await?;

            Ok(peer_connection)
        }
    }
}

pub trait Transport {
    fn add_ice_candidate(
        &self,
        candidate: RTCIceCandidateInit,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send;

    fn get_stats(&self) -> impl std::future::Future<Output = stats::StatsReport> + Send;

    fn signaling_state(&self) -> RTCSignalingState;

    fn ice_gathering_state(&self) -> RTCIceGatheringState;

    fn connection_state(&self) -> RTCPeerConnectionState;
}

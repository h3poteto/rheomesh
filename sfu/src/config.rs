use std::{collections::HashMap, fmt::Debug, net::IpAddr, sync::Arc, time::Duration};

use derivative::Derivative;
use local_ip_address::local_ip;
use webrtc::{
    api::setting_engine::SettingEngine, peer_connection::configuration::RTCConfiguration,
    rtp_transceiver::rtp_codec::RTCRtpCodecParameters, sdp::extmap,
};
use webrtc_ice::{
    network_type::NetworkType,
    udp_network::{EphemeralUDP, UDPNetwork},
};

const EXT_TOFFSET: &str = "urn:ietf:params:rtp-hdrext:toffset";
const EXT_PLAYOUT_DELAY: &str = "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay";
const EXT_VIDEO_CONTENT_TYPE: &str =
    "http://www.webrtc.org/experiments/rtp-hdrext/video-content-type";
const EXT_VIDEO_TIMING: &str = "http://www.webrtc.org/experiments/rtp-hdrext/video-timing";
const EXT_COLOR_SPACE: &str = "http://www.webrtc.org/experiments/rtp-hdrext/color-space";
const EXT_VIDEO_LAYERS_ALLOCATION00: &str =
    "http://www.webrtc.org/experiments/rtp-hdrext/video-layers-allocation00";
const EXT_FRAMEMARKING: &str = "urn:ietf:params:rtp-hdrext:framemarking";
const EXT_AV1_DEPENDENCY_DESCRIPTOR: &str =
    "https://aomediacodec.github.io/av1-rtp-spec/#dependency-descriptor-rtp-header-extension";

/// PortRange for [`WebRTCTransportConfig`]. In server side, random ports within this range are assigned for UDP connections.
#[derive(Clone, Debug)]
pub struct PortRange {
    /// Min port for UDP connections. It should be less than max.
    pub min: u16,
    /// Max port for UDP connections. It should be grather than min.
    pub max: u16,
}

/// Configuration for [`crate::publish_transport::PublishTransport`] and [`crate::subscribe_transport::SubscribeTransport`].
#[derive(Derivative)]
#[derivative(Clone, Debug)]
pub struct WebRTCTransportConfig {
    #[derivative(Debug = "ignore")]
    pub configuration: RTCConfiguration,
    pub announced_ips: Vec<IpAddr>,
    pub ice_disconnected_timeout: Option<Duration>,
    pub ice_failed_timeout: Option<Duration>,
    pub ice_keep_alive_interval: Option<Duration>,
    pub network_types: Vec<NetworkType>,
    pub ice_username_fragment: Option<String>,
    pub ice_password: Option<String>,
    pub port_range: Option<PortRange>,
}

impl Default for WebRTCTransportConfig {
    fn default() -> Self {
        Self {
            configuration: RTCConfiguration {
                ..Default::default()
            },
            announced_ips: vec![],
            ice_disconnected_timeout: None,
            ice_failed_timeout: None,
            ice_keep_alive_interval: None,
            network_types: vec![],
            ice_username_fragment: None,
            ice_password: None,
            port_range: None,
        }
    }
}

impl WebRTCTransportConfig {
    pub fn configuration(&self) -> RTCConfiguration {
        self.configuration.clone()
    }

    pub(crate) fn setting_engine(&self) -> SettingEngine {
        let mut setting_engine = SettingEngine::default();

        if self.ice_disconnected_timeout.is_some()
            || self.ice_failed_timeout.is_some()
            || self.ice_keep_alive_interval.is_some()
        {
            setting_engine.set_ice_timeouts(
                self.ice_disconnected_timeout,
                self.ice_failed_timeout,
                self.ice_keep_alive_interval,
            );
        }

        if self.announced_ips.len() > 0 {
            let announced_ips = Arc::new(self.announced_ips.clone());
            setting_engine.set_ip_filter(Box::new({
                let announced_ips = Arc::clone(&announced_ips);
                move |ip| announced_ips.contains(&ip)
            }));
        }

        if self.network_types.len() > 0 {
            setting_engine.set_network_types(self.network_types.clone());
        }

        if self.ice_username_fragment.is_some() || self.ice_password.is_some() {
            let username = self.ice_username_fragment.clone().unwrap_or("".to_string());
            let password = self.ice_password.clone().unwrap_or("".to_string());
            setting_engine.set_ice_credentials(username, password);
        }

        if let Some(port_range) = &self.port_range {
            let ephemeral = EphemeralUDP::new(port_range.min, port_range.max)
                .expect("failed to define ephemeral UDP");

            let udp_network = UDPNetwork::Ephemeral(ephemeral);
            setting_engine.set_udp_network(udp_network);
        }

        setting_engine
    }
}

/// Media configuration about codec and header extension for [`crate::router::Router`].
#[derive(Clone, Debug)]
pub struct MediaConfig {
    pub codec: CodecConfig,
    pub header_extension: HeaderExtensionConfig,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            codec: Default::default(),
            header_extension: Default::default(),
        }
    }
}

/// Media codec configuration for audio and video.
#[derive(Clone, Debug)]
pub struct CodecConfig {
    pub audio: Vec<RTCRtpCodecParameters>,
    pub video: Vec<RTCRtpCodecParameters>,
}

impl Default for CodecConfig {
    fn default() -> Self {
        Self {
            audio: Default::default(),
            video: Default::default(),
        }
    }
}

/// Header extension configuration for audio and video.
#[derive(Clone, Debug)]
pub struct HeaderExtensionConfig {
    pub audio: Vec<String>,
    pub video: Vec<String>,
}

impl Default for HeaderExtensionConfig {
    fn default() -> Self {
        Self {
            audio: vec![
                extmap::AUDIO_LEVEL_URI.to_owned(),
                extmap::ABS_SEND_TIME_URI.to_owned(),
                extmap::TRANSPORT_CC_URI.to_owned(),
                extmap::SDES_MID_URI.to_owned(),
            ],
            video: vec![
                EXT_TOFFSET.to_string(),
                extmap::SDES_MID_URI.to_owned(),
                extmap::SDES_RTP_STREAM_ID_URI.to_owned(),
                extmap::SDES_REPAIR_RTP_STREAM_ID_URI.to_owned(),
                extmap::ABS_SEND_TIME_URI.to_owned(),
                EXT_FRAMEMARKING.to_string(),
                EXT_AV1_DEPENDENCY_DESCRIPTOR.to_string(),
            ],
        }
    }
}

fn extmap_order() -> HashMap<u16, String> {
    HashMap::from([
        (1, extmap::AUDIO_LEVEL_URI.to_owned()),
        (2, extmap::ABS_SEND_TIME_URI.to_owned()),
        (3, extmap::TRANSPORT_CC_URI.to_owned()),
        (4, extmap::SDES_MID_URI.to_owned()),
        (5, EXT_PLAYOUT_DELAY.to_string()),
        (6, EXT_VIDEO_CONTENT_TYPE.to_string()),
        (7, EXT_VIDEO_TIMING.to_string()),
        (8, EXT_COLOR_SPACE.to_string()),
        (9, EXT_FRAMEMARKING.to_string()),
        (10, extmap::SDES_RTP_STREAM_ID_URI.to_owned()),
        (11, extmap::SDES_REPAIR_RTP_STREAM_ID_URI.to_owned()),
        (12, EXT_AV1_DEPENDENCY_DESCRIPTOR.to_string()),
        (13, extmap::VIDEO_ORIENTATION_URI.to_owned()),
        (14, EXT_TOFFSET.to_string()),
        (15, EXT_VIDEO_LAYERS_ALLOCATION00.to_string()),
    ])
}

pub(crate) fn find_extmap_order(uri: &str) -> Option<u16> {
    extmap_order()
        .into_iter()
        .find(|(_, v)| v == uri)
        .map(|(k, _)| k)
}

/// The RID is used to identify the video quality in a simulcast.
#[derive(Debug, Clone, strum::EnumString, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum RID {
    LOW,
    MID,
    HIGH,
}

impl From<RID> for u8 {
    fn from(value: RID) -> Self {
        match value {
            RID::LOW => 0,
            RID::MID => 1,
            RID::HIGH => 2,
        }
    }
}

impl From<u8> for RID {
    fn from(value: u8) -> Self {
        match value {
            0 => RID::LOW,
            1 => RID::MID,
            2 => RID::HIGH,
            _ => RID::HIGH,
        }
    }
}

/// Configuration for [`crate::worker::Worker`]. This configuration contains the ports for the relay server and sender.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// TCP port for the relay server. This is the port that the relay server will use to receive TCP messages from the sender.
    /// The default value is 9443.
    pub relay_server_tcp_port: u16,
    /// Private IP address of your server. This IP is used to communicate with other servers when relaying tracks.
    /// If your server is located in public subnet and it has a public IP address, you can set the public IP to this field.
    /// The default value is calculated from the server's network interface.
    pub private_ip: String,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        let ip = local_ip().unwrap();
        Self {
            relay_server_tcp_port: 9443,
            private_ip: ip.to_string(),
        }
    }
}

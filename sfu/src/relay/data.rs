use bincode::{Decode, Encode};
use bytes::{Bytes, BytesMut};

use serde::{Deserialize, Serialize};
use webrtc::{
    data_channel::data_channel_message::DataChannelMessage,
    rtp,
    rtp_transceiver::{
        rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters},
        PayloadType, RTCPFeedback,
    },
    util::marshal::Marshal,
};
use webrtc_util::Unmarshal;

use crate::{error::Error, publisher::PublisherType, rtp::layer::Layer};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct TrackData {
    pub(crate) router_id: String,
    pub(crate) track_id: String,
    pub(crate) ssrc: u32,
    pub(crate) codec_parameters: RTCRtpCodecParametersSerializable,
    pub(crate) stream_id: String,
    pub(crate) mime_type: String,
    pub(crate) rid: String,
    pub(crate) closed: bool,
    pub(crate) publisher_type: PublisherType,
    pub(crate) udp_port: Option<u16>,
    pub(crate) ip: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct RTCRtpCodecParametersSerializable {
    capability: RTCRtpCodecCapabilitySerializable,
    payload_type: PayloadType,
    stats_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct RTCRtpCodecCapabilitySerializable {
    mime_type: String,
    clock_rate: u32,
    channels: u16,
    sdp_fmtp_line: String,
    rtcp_feedback: Vec<RTCPFeedbackSerializable>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RTCPFeedbackSerializable {
    typ: String,
    parameter: String,
}

impl From<RTCPFeedback> for RTCPFeedbackSerializable {
    fn from(value: RTCPFeedback) -> Self {
        Self {
            typ: value.typ,
            parameter: value.parameter,
        }
    }
}

impl From<RTCPFeedbackSerializable> for RTCPFeedback {
    fn from(value: RTCPFeedbackSerializable) -> Self {
        Self {
            typ: value.typ,
            parameter: value.parameter,
        }
    }
}

impl From<RTCRtpCodecParameters> for RTCRtpCodecParametersSerializable {
    fn from(value: RTCRtpCodecParameters) -> Self {
        Self {
            capability: value.capability.into(),
            payload_type: value.payload_type,
            stats_id: value.stats_id,
        }
    }
}

impl From<RTCRtpCodecParametersSerializable> for RTCRtpCodecParameters {
    fn from(value: RTCRtpCodecParametersSerializable) -> Self {
        Self {
            capability: value.capability.into(),
            payload_type: value.payload_type,
            stats_id: value.stats_id,
        }
    }
}

impl From<RTCRtpCodecCapability> for RTCRtpCodecCapabilitySerializable {
    fn from(value: RTCRtpCodecCapability) -> Self {
        Self {
            mime_type: value.mime_type,
            clock_rate: value.clock_rate,
            channels: value.channels,
            sdp_fmtp_line: value.sdp_fmtp_line,
            rtcp_feedback: value.rtcp_feedback.into_iter().map(|f| f.into()).collect(),
        }
    }
}

impl From<RTCRtpCodecCapabilitySerializable> for RTCRtpCodecCapability {
    fn from(value: RTCRtpCodecCapabilitySerializable) -> Self {
        Self {
            mime_type: value.mime_type,
            clock_rate: value.clock_rate,
            channels: value.channels,
            sdp_fmtp_line: value.sdp_fmtp_line,
            rtcp_feedback: value.rtcp_feedback.into_iter().map(|f| f.into()).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, PartialEq)]
pub(crate) struct UDPStarted {
    pub(crate) port: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct PacketData {
    pub packet: rtp::packet::Packet,
    pub layer: Layer,
    pub ssrc: u32,
    pub track_id: String,
}

impl PacketData {
    pub fn marshal(&self) -> Result<Bytes, Error> {
        let packet_buf = self.packet.marshal()?;
        let layer_buf = self.layer.marshal();

        let mut ssrc_buf = BytesMut::with_capacity(4);
        ssrc_buf.extend_from_slice(&self.ssrc.to_be_bytes());

        let track_id_bytes = self.track_id.as_bytes();
        let track_id_len = track_id_bytes.len() as u8;
        let mut track_id_buf = BytesMut::with_capacity(track_id_len.into());
        track_id_buf.extend_from_slice(track_id_bytes);

        let mut track_len_buf = BytesMut::with_capacity(1);
        track_len_buf.extend_from_slice(&[track_id_len]);

        let buf = Bytes::from_iter(
            packet_buf.into_iter().chain(
                layer_buf.into_iter().chain(
                    ssrc_buf
                        .into_iter()
                        .chain(track_id_buf.into_iter().chain(track_len_buf.into_iter())),
                ),
            ),
        );

        Ok(buf)
    }

    pub fn unmarshal(bytes: &Bytes, len: usize) -> Result<Self, Error> {
        let track_id_len = bytes[len - 1] as usize;

        let layer_start_position = len - 1 - track_id_len - 6;

        let mut rtp_bytes = bytes.slice(..layer_start_position);
        let packet = rtp::packet::Packet::unmarshal(&mut rtp_bytes)?;

        let layer_bytes = bytes.slice(layer_start_position..layer_start_position + 2);
        let layer = Layer::unmarshal(&layer_bytes);

        let ssrc = u32::from_be_bytes(
            bytes[layer_start_position + 2..layer_start_position + 6]
                .try_into()
                .unwrap(),
        );

        let track_id =
            String::from_utf8(bytes[layer_start_position + 6..len - 1].to_vec()).unwrap();

        Ok(Self {
            packet,
            layer,
            ssrc,
            track_id,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MessageData {
    pub message: DataChannelMessage,
    pub data_publisher_id: String,
}

impl MessageData {
    pub fn marshal(&self) -> Result<Bytes, Error> {
        let data_publisher_id_bytes = self.data_publisher_id.as_bytes();
        let data_publisher_id_len = data_publisher_id_bytes.len() as u8;
        let mut id_len_buf = BytesMut::with_capacity(1);
        id_len_buf.extend_from_slice(&[data_publisher_id_len]);
        let mut id_buf = BytesMut::with_capacity(data_publisher_id_len.into());
        id_buf.extend_from_slice(data_publisher_id_bytes);

        let mut message_buf = BytesMut::with_capacity(self.message.data.len() + 1);
        message_buf.extend_from_slice(&[self.message.is_string as u8]);
        message_buf.extend_from_slice(&self.message.data);

        let buf = Bytes::from_iter(id_buf.into_iter().chain(message_buf.into_iter()));
        Ok(buf)
    }

    pub fn unmarshal(bytes: &Bytes, len: usize) -> Result<MessageData, Error> {
        let data_publisher_id_len = bytes[0] as usize;
        let id_bytes = bytes.slice(1..data_publisher_id_len);
        let data_publisher_id = String::from_utf8(id_bytes.to_vec()).unwrap();

        let is_string = bytes[1 + data_publisher_id_len] != 0;

        let data = bytes.slice(1 + data_publisher_id_len + 1..len);

        Ok(MessageData {
            message: DataChannelMessage { is_string, data },
            data_publisher_id,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChannelData {
    pub(crate) router_id: String,
    pub(crate) data_publisher_id: String,
    pub(crate) channel_id: u16,
    pub(crate) label: String,
    pub(crate) closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum RelayMessage {
    #[serde(rename = "trackdata")]
    Track(TrackData),
    #[serde(rename = "channeldata")]
    Channel(ChannelData),
}

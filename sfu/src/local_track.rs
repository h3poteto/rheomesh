use std::sync::Arc;
use std::time::Duration;

use enclose::enc;
use rtp::packetizer::Depacketizer;
use tokio::sync::{broadcast, mpsc};
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp::{self};
use webrtc::{
    rtp_transceiver::{rtp_receiver::RTCRtpReceiver, RTCRtpTransceiver},
    track::track_remote::TrackRemote,
};

use crate::publisher::PublisherEvent;
use crate::rtp::dependency_descriptor::DependencyDescriptorParser;
use crate::rtp::layer::Layer;
use crate::track::Track;
use crate::transport;

#[derive(Debug)]
pub struct LocalTrack {
    /// The ID is the same as published track_id.
    id: String,
    ssrc: u32,
    rid: String,
    track: Arc<TrackRemote>,
    _rtp_receiver: Arc<RTCRtpReceiver>,
    _rtp_transceiver: Arc<RTCRtpTransceiver>,
    rtcp_sender: Arc<transport::RtcpSender>,
    closed_sender: broadcast::Sender<bool>,
    rtp_packet_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
}

impl LocalTrack {
    pub(crate) fn new(
        track: Arc<TrackRemote>,
        rtp_receiver: Arc<RTCRtpReceiver>,
        rtp_transceiver: Arc<RTCRtpTransceiver>,
        rtcp_sender: Arc<transport::RtcpSender>,
        publisher_sender: mpsc::UnboundedSender<PublisherEvent>,
    ) -> Self {
        let track_id = track.id();
        let ssrc = track.ssrc();
        let rid = track.rid().to_string();

        let (sender, _reader) = broadcast::channel::<(rtp::packet::Packet, Layer)>(1024);
        let (tx, _rx) = broadcast::channel::<bool>(10);

        {
            let track_id = track_id.clone();
            let closed_sender = tx.clone();
            tokio::spawn(enc!((sender, track) async move {
                Self::rtp_event_loop(track_id, ssrc.clone(), sender, track, closed_sender).await;
                let _ = publisher_sender.send(PublisherEvent::TrackRemoved(ssrc));
            }));
        }

        {
            let closed_sender = tx.clone();
            let rtcp_sender = rtcp_sender.clone();
            let rid = rid.clone();
            tokio::spawn(async move {
                Self::pli_send_loop(rtcp_sender, ssrc, &rid, closed_sender).await;
            });
        }

        tracing::debug!("LocalTrack id={} ssrc={} is created", track_id, ssrc);

        let local_track = Self {
            id: track_id,
            ssrc,
            rid,
            track,
            _rtp_receiver: rtp_receiver,
            _rtp_transceiver: rtp_transceiver,
            rtcp_sender,
            closed_sender: tx,
            rtp_packet_sender: sender,
        };

        local_track
    }

    async fn rtp_event_loop(
        track_id: String,
        ssrc: u32,
        rtp_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
        track: Arc<TrackRemote>,
        closed_sender: broadcast::Sender<bool>,
    ) {
        tracing::debug!(
            "LocalTrack id={} ssrc={} RTP event loop has started, payload_type={}, mime_type={}",
            track_id,
            ssrc,
            track.payload_type(),
            track.codec().capability.mime_type
        );
        let mut local_track_closed = closed_sender.subscribe();
        drop(closed_sender);

        let mut last_timestamp = 0;
        let mut av1_parser = DependencyDescriptorParser::new();

        loop {
            tokio::select! {
                _closed = local_track_closed.recv() => {
                    break;
                }
                res = track.read_rtp() => {
                    match res {
                        Ok((mut rtp, _attr)) => {
                            let mut layer = Layer::new();
                            let payload_type = rtp.header.payload_type;
                            match payload_type {
                                96 => {
                                    // VP8 is 96.
                                    // https://github.com/webrtc-rs/webrtc/blob/b0630f4627c5722361b674b8b9f48ff509ea2113/webrtc/src/api/media_engine/mod.rs#L183
                                    let mut depacketizer = rtp::codecs::vp8::Vp8Packet::default();
                                    if let Ok(_payload) = depacketizer.depacketize(&rtp.payload) {
                                        layer.temporal_id = depacketizer.tid;
                                    }
                                }
                                98 | 100 => {
                                    // VP9 is 98 or 100.
                                    // https://github.com/webrtc-rs/webrtc/blob/b0630f4627c5722361b674b8b9f48ff509ea2113/webrtc/src/api/media_engine/mod.rs#L194
                                    // https://github.com/webrtc-rs/webrtc/blob/b0630f4627c5722361b674b8b9f48ff509ea2113/webrtc/src/api/media_engine/mod.rs#L205
                                    let mut depacketizer = rtp::codecs::vp9::Vp9Packet::default();
                                    if let Ok(_payload) = depacketizer.depacketize(&rtp.payload) {
                                        layer.temporal_id = depacketizer.tid;
                                        layer.spatial_id = depacketizer.sid;
                                    }
                                }
                                41 | 45 | 102  | 125 | 108 | 127 | 123 => {
                                    // AV1 is 41
                                    // https://github.com/webrtc-rs/webrtc/blob/b0630f4627c5722361b674b8b9f48ff509ea2113/webrtc/src/api/media_engine/mod.rs#L294
                                    // But, sometimes we receive AV1 with payload_type: 45

                                    // H.264 doesn't have tid in the packet header.
                                    // https://docs.rs/rtp/0.12.0/rtp/codecs/h264/struct.H264Packet.html
                                    // Instead, H.264 has the same dependency descriptor header.
                                    for ext in rtp.header.extensions.iter() {
                                        if ext.id == 12 {
                                            if let Some(dd) = av1_parser.parse(&ext.payload) {
                                                layer.temporal_id = dd.temporal_id;
                                                layer.spatial_id = dd.spatial_id;
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }

                            let old_timestamp = rtp.header.timestamp;
                            if last_timestamp == 0 {
                                rtp.header.timestamp = 0
                            } else if rtp.header.timestamp < last_timestamp {
                                rtp.header.timestamp = 0
                            } else {
                                rtp.header.timestamp -= last_timestamp;
                            }
                            last_timestamp = old_timestamp;

                            tracing::trace!(
                                "LocalTrack id={} received RTP ssrc={} seq={} timestamp={}",
                                track_id,
                                rtp.header.ssrc,
                                rtp.header.sequence_number,
                                rtp.header.timestamp
                            );

                            if rtp_sender.receiver_count() > 0 {
                                if let Err(err) = rtp_sender.send((rtp, layer)) {
                                    tracing::error!("LocalTrack id={} ssrc={} failed to send rtp: {}", track_id, ssrc, err);
                                }
                            }
                        }
                        Err(webrtc::error::Error::ErrDataChannelNotOpen) => {
                            break;
                        }
                        Err(webrtc::error::Error::ErrClosedPipe) =>{
                            break;
                        }
                        Err(webrtc::error::Error::Interceptor(webrtc::interceptor::Error::Srtp(webrtc_srtp::Error::Util(webrtc_util::Error::ErrBufferClosed)))) => {
                            break;
                        }
                        Err(err) => {
                            tracing::error!("LocalTrack id={} ssrc={} failed to read rtp: {:#?}", track_id, ssrc, err);
                            break;
                        }
                    }
                }
            }
        }

        tracing::debug!(
            "LocalTrack id={} ssrc={} RTP event loop has finished",
            track_id,
            ssrc
        );
    }

    async fn pli_send_loop(
        rtcp_sender: Arc<transport::RtcpSender>,
        media_ssrc: u32,
        rid: &str,
        closed_sender: broadcast::Sender<bool>,
    ) {
        tracing::debug!(
            "Sending pli for stream with ssrc={}, rid={}",
            media_ssrc,
            rid
        );
        let mut local_track_closed = closed_sender.subscribe();
        drop(closed_sender);

        loop {
            let timeout = tokio::time::sleep(Duration::from_secs(3));
            tokio::pin!(timeout);

            tokio::select! {
                _closed = local_track_closed.recv() => {
                    break;
                }
                _ = timeout.as_mut() => {
                    match rtcp_sender.send(Box::new(PictureLossIndication {
                        sender_ssrc: 0,
                        media_ssrc,
                    })) {
                        Ok(_) => tracing::trace!("sent rtcp pli ssrc={}, rid={}", media_ssrc, rid),
                        Err(err) => tracing::error!("LocalTrack failed to send rtcp pli ssrc={}, rid={}, {}", media_ssrc, rid, err)
                    }
                }

            };
        }
        tracing::debug!(
            "Finish sending pli for stream with ssrc: {}, rid: {}",
            media_ssrc,
            rid
        );
    }
}

impl Track for LocalTrack {
    fn rtcp_sender(&self) -> Arc<transport::RtcpSender> {
        self.rtcp_sender.clone()
    }

    fn rtp_packet_sender(&self) -> broadcast::Sender<(rtp::packet::Packet, Layer)> {
        self.rtp_packet_sender.clone()
    }

    fn mime_type(&self) -> String {
        self.track.codec().capability.mime_type.clone()
    }

    fn capability(&self) -> webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
        self.track.codec().capability
    }

    fn id(&self) -> String {
        self.track.id()
    }

    fn stream_id(&self) -> String {
        self.track.stream_id()
    }

    fn ssrc(&self) -> u32 {
        self.track.ssrc()
    }

    fn rid(&self) -> String {
        self.rid.clone()
    }

    fn close(&self) {
        self.closed_sender.send(true).unwrap();
    }
}

// pub(crate) fn detect_mime_type(mime_type: String) -> MediaType {
//     if mime_type.contains("video") || mime_type.contains("Video") {
//         MediaType::Video
//     } else {
//         MediaType::Audio
//     }
// }

// pub(crate) enum MediaType {
//     Video,
//     Audio,
// }

impl Drop for LocalTrack {
    fn drop(&mut self) {
        tracing::debug!("LocalTrack id={} ssrc={} is dropped", self.id, self.ssrc);
    }
}

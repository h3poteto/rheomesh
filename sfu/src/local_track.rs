use std::sync::atomic::Ordering;
use std::sync::{Arc, atomic::AtomicU8};
use std::time::Duration;

use derivative::Derivative;
use enclose::enc;
use rtc::{
    media_stream::MediaStreamId,
    rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication,
    rtp,
    rtp_transceiver::{
        PayloadType,
        rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters},
    },
};
use rtp::packetizer::Depacketizer;
use tokio::sync::{
    broadcast,
    mpsc::{self},
};
use webrtc::media_stream::track_remote::TrackRemote;

use crate::publisher::PublisherEvent;
use crate::rtp::dependency_descriptor::DependencyDescriptorParser;
use crate::rtp::layer::Layer;
use crate::track::Track;
use crate::transport;

#[derive(Derivative)]
#[derivative(Debug)]
pub struct LocalTrack {
    /// The ID is the same as published track_id.
    id: String,
    ssrc: u32,
    rid: String,
    codec: Option<RTCRtpCodec>,
    payload_type: Arc<AtomicU8>,
    stream_id: MediaStreamId,
    rtcp_sender: Arc<transport::RtcpSender>,
    closed_sender: broadcast::Sender<bool>,
    rtp_packet_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
    rtp_input_sender: mpsc::UnboundedSender<rtp::packet::Packet>,
}

impl LocalTrack {
    pub(crate) async fn new(
        track_id: String,
        ssrc: u32,
        rid: String,
        track: Arc<dyn TrackRemote>,
        publisher_sender: mpsc::UnboundedSender<PublisherEvent>,
    ) -> Self {
        let (rtcp_sender, rtcp_receiver) = mpsc::unbounded_channel();
        let rtcp_sender = Arc::new(rtcp_sender);

        let (sender, _reader) = broadcast::channel::<(rtp::packet::Packet, Layer)>(1024);
        let (tx, _rx) = broadcast::channel::<bool>(10);

        let (rtp_input_sender, rtp_input_receiver) = mpsc::unbounded_channel();

        let payload_type = Arc::new(AtomicU8::new(0));

        {
            let track_id = track_id.clone();
            let closed_sender = tx.clone();
            let mime_type = track.codec(ssrc).await.unwrap_or_default().mime_type;
            tokio::spawn(enc!((sender, payload_type) async move {
                Self::rtp_event_loop(track_id, ssrc.clone(), mime_type, rtp_input_receiver, sender, closed_sender, payload_type).await;
                let _ = publisher_sender.send(PublisherEvent::TrackRemoved(ssrc));
            }));
        }

        {
            let closed_sender = tx.clone();
            let rid = rid.clone();
            tokio::spawn(enc!((track) async move {
                Self::pli_send_loop(track, ssrc, &rid, closed_sender).await;
            }));
        }

        {
            let tx = tx.clone();
            tokio::spawn(enc!((track) async move {
                let closed = tx.subscribe();
                Self::rtcp_writer_loop(track, rtcp_receiver, closed).await;
            }));
        }

        tracing::debug!("LocalTrack id={} ssrc={} is created", track_id, ssrc);

        let local_track = Self {
            id: track_id,
            ssrc,
            rid,
            codec: track.codec(ssrc).await,
            payload_type,
            stream_id: track.stream_id().await,
            rtcp_sender,
            closed_sender: tx,
            rtp_packet_sender: sender,
            rtp_input_sender,
        };

        local_track
    }

    async fn rtp_event_loop(
        track_id: String,
        ssrc: u32,
        mime_type: String,
        mut rtp_input_receiver: mpsc::UnboundedReceiver<rtp::packet::Packet>,
        rtp_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
        closed_sender: broadcast::Sender<bool>,
        pt: Arc<AtomicU8>,
    ) {
        tracing::debug!(
            "LocalTrack id={} ssrc={} RTP event loop has started, mime_type={}",
            track_id,
            ssrc,
            mime_type,
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
                packet = rtp_input_receiver.recv() => {
                    match packet {
                        Some(mut rtp) => {
                            let mut layer = Layer::new();
                            let payload_type = rtp.header.payload_type;
                            pt.store(payload_type, Ordering::Relaxed);
                            match payload_type {
                                96 => {
                                    // VP8 is 96.
                                    // https://github.com/webrtc-rs/webrtc/blob/b0630f4627c5722361b674b8b9f48ff509ea2113/webrtc/src/api/media_engine/mod.rs#L183
                                    let mut depacketizer = rtp::codec::vp8::Vp8Packet::default();
                                    if let Ok(_payload) = depacketizer.depacketize(&rtp.payload) {
                                        layer.temporal_id = depacketizer.tid;
                                    }
                                }
                                98 | 100 => {
                                    // VP9 is 98 or 100.
                                    // https://github.com/webrtc-rs/webrtc/blob/b0630f4627c5722361b674b8b9f48ff509ea2113/webrtc/src/api/media_engine/mod.rs#L194
                                    // https://github.com/webrtc-rs/webrtc/blob/b0630f4627c5722361b674b8b9f48ff509ea2113/webrtc/src/api/media_engine/mod.rs#L205
                                    let mut depacketizer = rtp::codec::vp9::Vp9Packet::default();
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
                        None => break
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
        track: Arc<dyn TrackRemote>,
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
                    let pli = Box::new(PictureLossIndication {
                        sender_ssrc: 0,
                        media_ssrc,
                    });
                    match track.write_rtcp(vec![pli]).await {
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

    async fn rtcp_writer_loop(
        track: Arc<dyn TrackRemote>,
        mut rtcp_receiver: transport::RtcpReceiver,
        mut closed: broadcast::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                Some(packet) = rtcp_receiver.recv() => {
                    if let Err(err) = track.write_rtcp(vec![packet]).await {
                        tracing::error!("Error writing RTCP: {}", err);
                    }
                },
                _ = closed.recv() => break,
            }
        }
    }

    pub(crate) fn rtp_input_sender(&self) -> mpsc::UnboundedSender<rtp::packet::Packet> {
        self.rtp_input_sender.clone()
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
        self.codec.clone().unwrap_or_default().mime_type.clone()
    }

    fn payload_type(&self) -> PayloadType {
        self.payload_type.load(Ordering::Relaxed)
    }

    fn parameters(&self) -> RTCRtpCodecParameters {
        RTCRtpCodecParameters {
            rtp_codec: self.codec.clone().unwrap_or_default(),
            payload_type: self.payload_type(),
        }
    }

    fn capability(&self) -> RTCRtpCodec {
        self.codec.clone().unwrap_or_default()
    }

    fn id(&self) -> String {
        self.id.clone()
    }

    fn stream_id(&self) -> MediaStreamId {
        self.stream_id.clone()
    }

    fn ssrc(&self) -> u32 {
        self.ssrc.clone()
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

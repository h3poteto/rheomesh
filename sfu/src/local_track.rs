use std::sync::Arc;
use std::time::Duration;

use enclose::enc;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::time::sleep;
use webrtc::rtp;
use webrtc::{
    rtp_transceiver::{rtp_receiver::RTCRtpReceiver, RTCRtpTransceiver},
    track::track_remote::TrackRemote,
};

use crate::publisher::PublisherEvent;
use crate::transport;

#[derive(Debug)]
pub struct LocalTrack {
    /// The ID is the same as published track_id.
    pub id: String,
    pub ssrc: u32,
    pub track: Arc<TrackRemote>,
    _rtp_receiver: Arc<RTCRtpReceiver>,
    _rtp_transceiver: Arc<RTCRtpTransceiver>,
    pub(crate) rtcp_sender: Arc<transport::RtcpSender>,
    closed_sender: Arc<mpsc::UnboundedSender<bool>>,
    pub(crate) rtp_packet_sender: broadcast::Sender<rtp::packet::Packet>,
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

        let (sender, _reader) = broadcast::channel::<rtp::packet::Packet>(1024);
        let (tx, rx) = mpsc::unbounded_channel();

        {
            let track_id = track_id.clone();
            let closed_receiver = Arc::new(Mutex::new(rx));
            tokio::spawn(enc!((sender, track) async move {
                Self::rtp_event_loop(track_id, ssrc.clone(), sender, track, closed_receiver).await;
                let _ = publisher_sender.send(PublisherEvent::TrackRemoved(ssrc));
            }));
        }

        tracing::debug!("LocalTrack id={} ssrc={} is created", track_id, ssrc);

        let local_track = Self {
            id: track_id,
            ssrc,
            track,
            _rtp_receiver: rtp_receiver,
            _rtp_transceiver: rtp_transceiver,
            rtcp_sender,
            closed_sender: Arc::new(tx),
            rtp_packet_sender: sender,
        };

        local_track
    }

    async fn rtp_event_loop(
        track_id: String,
        ssrc: u32,
        rtp_sender: broadcast::Sender<rtp::packet::Packet>,
        track: Arc<TrackRemote>,
        loacl_track_closed: Arc<Mutex<mpsc::UnboundedReceiver<bool>>>,
    ) {
        tracing::debug!(
            "LocalTrack id={} ssrc={} RTP event loop has started, payload_type={}, mime_type={}",
            track_id,
            ssrc,
            track.payload_type(),
            track.codec().capability.mime_type
        );

        let mut last_timestamp = 0;

        loop {
            let mut local_track_closed = loacl_track_closed.lock().await;
            tokio::select! {
                _closed = local_track_closed.recv() => {
                    break;
                }
                res = track.read_rtp() => {
                    match res {
                        Ok((mut rtp, _attr)) => {
                            let old_timestamp = rtp.header.timestamp;
                            if last_timestamp == 0 {
                                rtp.header.timestamp = 0
                            } else {
                                rtp.header.timestamp -= last_timestamp;
                            }
                            last_timestamp = old_timestamp;

                            tracing::trace!(
                                "LocalTrack id={} ssrc={} received RTP ssrc={} seq={} timestamp={}",
                                track_id,
                                ssrc,
                                rtp.header.ssrc,
                                rtp.header.sequence_number,
                                rtp.header.timestamp
                            );

                            if rtp_sender.receiver_count() > 0 {
                                if let Err(err) = rtp_sender.send(rtp) {
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
            sleep(Duration::from_millis(1)).await;
        }

        tracing::debug!(
            "LocalTrack id={} ssrc={} RTP event loop has finished",
            track_id,
            ssrc
        );
    }

    pub(crate) async fn close(&self) {
        self.closed_sender.send(true).unwrap();
    }
}

pub(crate) fn detect_mime_type(mime_type: String) -> MediaType {
    if mime_type.contains("video") || mime_type.contains("Video") {
        MediaType::Video
    } else {
        MediaType::Audio
    }
}

pub(crate) enum MediaType {
    Video,
    Audio,
}

impl Drop for LocalTrack {
    fn drop(&mut self) {
        tracing::debug!("LocalTrack id={} ssrc={} is dropped", self.id, self.ssrc);
    }
}

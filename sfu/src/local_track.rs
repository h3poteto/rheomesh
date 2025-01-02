use std::sync::Arc;
use std::time::Duration;

use enclose::enc;
use tokio::sync::{broadcast, mpsc};
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
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
    pub rid: String,
    pub track: Arc<TrackRemote>,
    _rtp_receiver: Arc<RTCRtpReceiver>,
    _rtp_transceiver: Arc<RTCRtpTransceiver>,
    pub(crate) rtcp_sender: Arc<transport::RtcpSender>,
    closed_sender: broadcast::Sender<bool>,
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
        let rid = track.rid().to_string();

        let (sender, _reader) = broadcast::channel::<rtp::packet::Packet>(1024);
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
        rtp_sender: broadcast::Sender<rtp::packet::Packet>,
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

        loop {
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
                                "LocalTrack id={} received RTP ssrc={} seq={} timestamp={}",
                                track_id,
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

    pub(crate) async fn close(&self) {
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

use std::sync::{
    atomic::{AtomicU16, AtomicU32, Ordering},
    Arc,
};

use enclose::enc;
use tokio::sync::{broadcast, mpsc, Mutex};
use uuid::Uuid;
use webrtc::{
    rtcp::{
        self,
        header::{PacketType, FORMAT_PLI},
        payload_feedbacks::picture_loss_indication::PictureLossIndication,
    },
    rtp,
    rtp_transceiver::rtp_sender::RTCRtpSender,
    track::track_local::{track_local_static_rtp::TrackLocalStaticRTP, TrackLocalWriter},
};

use crate::{
    config::RID,
    error::Error,
    router::{Router, RouterEvent},
    transport,
};

#[derive(Debug)]
pub struct Subscriber {
    pub id: String,
    publisher_id: String,
    closed_sender: broadcast::Sender<bool>,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    track_local: Arc<TrackLocalStaticRTP>,
    publisher_rtcp_sender: Arc<transport::RtcpSender>,
    rtcp_sender: Arc<RTCRtpSender>,
    sequence: Arc<AtomicU16>,
    timestamp: Arc<AtomicU32>,
    rtp_lock: Arc<Mutex<bool>>,
    rtcp_lock: Arc<Mutex<bool>>,
}

impl Subscriber {
    pub(crate) fn new(
        publisher_id: String,
        track_local: Arc<TrackLocalStaticRTP>,
        rtp_sender: broadcast::Sender<rtp::packet::Packet>,
        rtcp_sender: Arc<RTCRtpSender>,
        publisher_rtcp_sender: Arc<transport::RtcpSender>,
        _mime_type: String,
        media_ssrc: u32,
        router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    ) -> Self {
        let id = Uuid::new_v4().to_string();
        let (tx, _rx) = broadcast::channel::<bool>(1);

        let sequence = Arc::new(AtomicU16::new(0));
        let timestamp = Arc::new(AtomicU32::new(0));
        let rtp_lock = Arc::new(Mutex::new(true));

        tokio::spawn(
            enc!((id, media_ssrc, track_local, rtp_sender, tx, publisher_rtcp_sender, rtp_lock, sequence, timestamp) async move {
                Self::rtp_event_loop(
                    id,
                    media_ssrc,
                    track_local,
                    rtp_sender,
                    tx,
                    publisher_rtcp_sender,
                    rtp_lock,
                    sequence,
                    timestamp
                )
                .await;
            }),
        );

        let rtcp_lock = Arc::new(Mutex::new(true));

        tokio::spawn(
            enc!((rtcp_sender, publisher_rtcp_sender, id, media_ssrc, tx, rtcp_lock) async move {
                Self::rtcp_event_loop(id, media_ssrc, rtcp_sender, publisher_rtcp_sender, tx, rtcp_lock).await;
            }),
        );

        tracing::debug!(
            "Subscriber id={} is created for publisher_ssrc={}",
            id,
            media_ssrc
        );

        Self {
            publisher_id,
            id,
            closed_sender: tx,
            router_event_sender,
            track_local,
            publisher_rtcp_sender,
            rtcp_sender,
            sequence,
            timestamp,
            rtp_lock,
            rtcp_lock,
        }
    }

    pub async fn set_preferred_layer(&self, rid: RID) -> Result<(), Error> {
        let local_track = Router::find_local_track(
            self.router_event_sender.clone(),
            self.publisher_id.clone(),
            rid,
        )
        .await?;

        {
            let id = self.id.clone();
            let ssrc = local_track.ssrc.clone();
            let track_local = self.track_local.clone();
            let rtp_sender = local_track.rtp_packet_sender.clone();
            let closed_sender = self.closed_sender.clone();
            let publisher_rtcp_sender = self.publisher_rtcp_sender.clone();
            let rtp_lock = self.rtp_lock.clone();
            let sequence = self.sequence.clone();
            let timestamp = self.timestamp.clone();
            tokio::spawn(async move {
                Self::rtp_event_loop(
                    id,
                    ssrc,
                    track_local,
                    rtp_sender,
                    closed_sender,
                    publisher_rtcp_sender,
                    rtp_lock,
                    sequence,
                    timestamp,
                )
                .await;
            });
        }
        {
            let id = self.id.clone();
            let ssrc = local_track.ssrc.clone();
            let rtcp_sender = self.rtcp_sender.clone();
            let closed_sender = self.closed_sender.clone();
            let publisher_rtcp_sender = self.publisher_rtcp_sender.clone();
            let loop_lock = self.rtcp_lock.clone();
            tokio::spawn(async move {
                Self::rtcp_event_loop(
                    id,
                    ssrc,
                    rtcp_sender,
                    publisher_rtcp_sender,
                    closed_sender,
                    loop_lock,
                )
                .await;
            });
        }

        let _ = self.closed_sender.send(true);

        Ok(())
    }

    pub(crate) async fn rtp_event_loop(
        id: String,
        media_ssrc: u32,
        track_local: Arc<TrackLocalStaticRTP>,
        rtp_sender: broadcast::Sender<rtp::packet::Packet>,
        subscriber_closed_sender: broadcast::Sender<bool>,
        publisher_rtcp_sender: Arc<transport::RtcpSender>,
        loop_lock: Arc<Mutex<bool>>,
        init_sequence: Arc<AtomicU16>,
        init_timestamp: Arc<AtomicU32>,
    ) {
        let mut _gurad = loop_lock.lock().await;

        let mut rtp_receiver = rtp_sender.subscribe();
        drop(rtp_sender);
        let mut subscriber_closed = subscriber_closed_sender.subscribe();
        drop(subscriber_closed_sender);

        tracing::debug!(
            "Subscriber id={} publisher_ssrc={} RTP event loop has started",
            id,
            media_ssrc
        );

        let mut current_timestamp = init_timestamp.load(Ordering::Relaxed);
        let mut last_sequence_number: u16 = init_sequence.load(Ordering::Relaxed);

        loop {
            tokio::select! {
                _ = subscriber_closed.recv() => {
                    break;
                }
                res = rtp_receiver.recv() => {
                    if publisher_rtcp_sender.is_closed() {
                        break;
                    }
                    match res {
                        Ok(mut packet) => {
                            current_timestamp += packet.header.timestamp;
                            packet.header.timestamp = current_timestamp;
                            last_sequence_number = last_sequence_number.wrapping_add(1);
                            packet.header.sequence_number = last_sequence_number;

                            tracing::trace!(
                                "Subscriber id={} write RTP ssrc={} seq={} timestamp={}",
                                id,
                                packet.header.ssrc,
                                packet.header.sequence_number,
                                packet.header.timestamp
                            );


                            if let Err(err) = track_local.write_rtp(&packet).await {
                                tracing::error!("Subscriber id={} failed to write rtp: {}", id, err)
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            break;
                        }
                        Err(err) => {
                            tracing::error!("Subscriber id={} failed to read rtp: {}", id, err);
                        }
                    }
                }
            }
        }

        init_sequence.store(last_sequence_number, Ordering::Relaxed);
        init_timestamp.store(current_timestamp, Ordering::Relaxed);

        tracing::debug!(
            "Subscriber id={} publisher_ssrc={} RTP event loop has finished",
            id,
            media_ssrc
        );
    }

    pub(crate) async fn rtcp_event_loop(
        id: String,
        media_ssrc: u32,
        rtcp_sender: Arc<RTCRtpSender>,
        publisher_rtcp_sender: Arc<transport::RtcpSender>,
        subscriber_closed_sender: broadcast::Sender<bool>,
        loop_lock: Arc<Mutex<bool>>,
    ) {
        let mut _guard = loop_lock.lock().await;

        let mut subscriber_closed = subscriber_closed_sender.subscribe();
        drop(subscriber_closed_sender);

        tracing::debug!(
            "Subscriber id={} publisher_ssrc={} RTCP event loop has started",
            id,
            media_ssrc
        );

        loop {
            tokio::select! {
                _ = subscriber_closed.recv() => {
                    break;
                }
                res = rtcp_sender.read_rtcp() => {
                    if publisher_rtcp_sender.is_closed() {
                        break;
                    }
                    match res {
                        Ok((rtcp_packets, attr)) => {
                            for rtcp in rtcp_packets.into_iter() {
                                tracing::trace!("Receive RTCP subscriber={} rtcp={:#?}, attr={:#?}", id, rtcp, attr);

                                let header = rtcp.header();
                                match header.packet_type {
                                    PacketType::ReceiverReport => {
                                        if let Some(_rr) = rtcp
                                            .as_any()
                                            .downcast_ref::<rtcp::receiver_report::ReceiverReport>()
                                        {
                                            // Received receiver reports.
                                        }
                                    }
                                    PacketType::PayloadSpecificFeedback => match header.count {
                                        FORMAT_PLI => {
                                            if let Some(_pli) = rtcp.as_any().downcast_ref::<rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>() {
                                                match publisher_rtcp_sender.send(Box::new(PictureLossIndication {
                                                    sender_ssrc: 0,
                                                    media_ssrc,
                                                })) {
                                                    Ok(_) => tracing::trace!("send rtcp: pli"),
                                                    Err(err) => tracing::error!("Subscriber id ={} failed to send rtcp pli: {}", id, err)
                                                }
                                            }
                                        }
                                        _ => {}
                                    },
                                    _ => {}
                                }
                            }

                        }
                        Err(webrtc::error::Error::ErrDataChannelNotOpen) => {
                            break;
                        }
                        Err(webrtc::error::Error::ErrClosedPipe) => {
                            break;
                        }
                        Err(err) => {
                            tracing::error!("Subscriber id={} failed to read rtcp: {:#?}", id, err);
                        }
                    }
                }
            }
        }

        tracing::debug!(
            "Subscriber id={} publisher_ssrc={} RTCP event loop finished",
            id,
            media_ssrc
        );
    }

    pub async fn close(&self) {
        self.closed_sender.send(true).unwrap();
    }
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        tracing::debug!("Subscriber id={} is dropped", self.id);
    }
}

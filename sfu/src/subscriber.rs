use std::{sync::Arc, time::Duration};

use chrono::Utc;
use enclose::enc;
use tokio::{sync::broadcast, time::sleep};
use uuid::Uuid;
use webrtc::{
    rtcp::{
        self,
        header::{PacketType, FORMAT_PLI, FORMAT_REMB},
        payload_feedbacks::picture_loss_indication::PictureLossIndication,
    },
    rtp,
    rtp_transceiver::rtp_sender::RTCRtpSender,
    track::track_local::{track_local_static_rtp::TrackLocalStaticRTP, TrackLocalWriter},
};

use crate::{
    publisher::{detect_mime_type, MediaType},
    transport,
};

#[derive(Clone, Debug)]
pub struct Subscriber {
    pub id: String,
    closed_sender: broadcast::Sender<bool>,
}

impl Subscriber {
    pub(crate) fn new(
        local_track: Arc<TrackLocalStaticRTP>,
        rtp_sender: broadcast::Sender<rtp::packet::Packet>,
        rtcp_sender: Arc<RTCRtpSender>,
        publisher_rtcp_sender: Arc<transport::RtcpSender>,
        mime_type: String,
        media_ssrc: u32,
    ) -> Self {
        let id = Uuid::new_v4().to_string();
        let (tx, _rx) = broadcast::channel::<bool>(1);

        {
            let tx = tx.clone();
            let id = id.clone();
            let media_ssrc = media_ssrc.clone();
            tokio::spawn(async move {
                Self::rtp_event_loop(id, media_ssrc, local_track, rtp_sender, tx).await;
            });
        }

        {
            let tx = tx.clone();
            let id = id.clone();
            let media_ssrc = media_ssrc.clone();
            tokio::spawn(enc!((rtcp_sender, publisher_rtcp_sender) async move {
                Self::rtcp_event_loop(id, media_ssrc, rtcp_sender, publisher_rtcp_sender, mime_type, tx).await;
            }));
        }

        tracing::debug!(
            "Subscriber id={} is created for publisher_ssrc={}",
            id,
            media_ssrc
        );

        Self {
            id,
            closed_sender: tx,
        }
    }

    pub(crate) async fn rtp_event_loop(
        id: String,
        media_ssrc: u32,
        local_track: Arc<TrackLocalStaticRTP>,
        rtp_sender: broadcast::Sender<rtp::packet::Packet>,
        subscriber_closed_sender: broadcast::Sender<bool>,
    ) {
        let mut rtp_receiver = rtp_sender.subscribe();
        drop(rtp_sender);
        let mut subscriber_closed = subscriber_closed_sender.subscribe();
        drop(subscriber_closed_sender);

        tracing::debug!(
            "Subscriber id={} publisher_ssrc={} RTP event loop has started",
            id,
            media_ssrc
        );

        let mut current_timestamp = 0;

        loop {
            tokio::select! {
                _ = subscriber_closed.recv() => {
                    break;
                }
                res = rtp_receiver.recv() => {
                    match res {
                        Ok(mut packet) => {
                            current_timestamp += packet.header.timestamp;
                            packet.header.timestamp = current_timestamp;

                            tracing::trace!(
                                "Subscriber id={} write RTP ssrc={} seq={} timestamp={}",
                                id,
                                packet.header.ssrc,
                                packet.header.sequence_number,
                                packet.header.timestamp
                            );


                            if let Err(err) = local_track.write_rtp(&packet).await {
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
                    sleep(Duration::from_millis(1)).await;
                }
            }
        }

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
        mime_type: String,
        subscriber_closed_sender: broadcast::Sender<bool>,
    ) {
        let mut subscriber_closed = subscriber_closed_sender.subscribe();
        drop(subscriber_closed_sender);

        let media_type = detect_mime_type(mime_type);
        let start_timestamp = Utc::now();
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
                    match res {
                        Ok((rtcp_packets, attr)) => {
                            for rtcp in rtcp_packets.into_iter() {
                                tracing::trace!("Receive RTCP subscriber={} rtcp={:#?}, attr={:#?}", id, rtcp, attr);

                                let header = rtcp.header();
                                match header.packet_type {
                                    PacketType::ReceiverReport => {
                                        if let Some(rr) = rtcp
                                            .as_any()
                                            .downcast_ref::<rtcp::receiver_report::ReceiverReport>()
                                        {
                                            let rr = rr.clone();
                                            match publisher_rtcp_sender.send(Box::new(rr)) {
                                                Ok(_) => tracing::trace!("send rtcp: rr"),
                                                Err(err) => tracing::error!("Subscriber id={} failed to send rtcp rr: {}", id, err),
                                            }
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
                                        FORMAT_REMB => {
                                            if let Some(remb) = rtcp.as_any().downcast_ref::<rtcp::payload_feedbacks::receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate>() {

                                                let mut remb = remb.clone();
                                                let diff = Utc::now() - start_timestamp;
                                                if diff.num_seconds() < 30 {
                                                    // Min bitrate is 128kbps if it is video and first 30seconds.
                                                    match media_type {
                                                        MediaType::Video => {
                                                            if remb.bitrate < 128000.0 {
                                                                remb.bitrate = 128000.0;
                                                            }
                                                        }
                                                        MediaType::Audio => {
                                                            if remb.bitrate < 64000.0 {
                                                                remb.bitrate = 640000.0
                                                            }
                                                        }
                                                    }
                                                }

                                                match publisher_rtcp_sender.send(Box::new(remb)) {
                                                    Ok(_) => tracing::trace!("send rtcp: remb"),
                                                    Err(err) => tracing::error!("Subscriber id ={} failed to send rtcp remb: {}", id, err)
                                                }
                                            }
                                        }
                                        _ => {}
                                    },
                                    _ => {}
                                }
                            }

                        }
                        Err(err) => {
                            tracing::error!("Subscriber id={} failed to read rtcp: {}", id, err);
                            if webrtc::error::Error::ErrDataChannelNotOpen == err {
                                break;
                            }
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

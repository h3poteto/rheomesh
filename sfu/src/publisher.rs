use std::{collections::HashMap, sync::Arc};

use derivative::Derivative;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use webrtc::{
    rtp_transceiver::{rtp_receiver::RTCRtpReceiver, RTCRtpTransceiver},
    track::track_remote::TrackRemote,
};

use crate::{
    config::RID,
    error::{Error, PublisherErrorKind},
    local_track::LocalTrack,
    relay::sender::RelaySender,
    router::RouterEvent,
    subscriber::SubscriberEvent,
    track::Track,
    transport,
};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct Publisher {
    pub track_id: String,
    local_tracks: HashMap<u32, Arc<LocalTrack>>,
    router_sender: mpsc::UnboundedSender<RouterEvent>,
    publisher_event_sender: mpsc::UnboundedSender<PublisherEvent>,
    #[derivative(Debug = "ignore")]
    close_callback: Box<dyn Fn(String) + Send + Sync>,
    rid_to_ssrc: HashMap<String, u32>,
    pub publisher_type: PublisherType,
    subscriber_event_sender: Vec<mpsc::UnboundedSender<SubscriberEvent>>,
    relay_sender: Arc<RelaySender>,
    relayed_targets: Vec<(String, u16, String)>,
    relayed_publishers: HashMap<u32, String>,
}

impl Publisher {
    pub(crate) fn new(
        track_id: String,
        router_sender: mpsc::UnboundedSender<RouterEvent>,
        publisher_type: PublisherType,
        relay_sender: Arc<RelaySender>,
        close_callback: Box<dyn Fn(String) + Send + Sync>,
    ) -> Arc<Mutex<Publisher>> {
        let (tx, rx) = mpsc::unbounded_channel::<PublisherEvent>();

        let publisher = Self {
            track_id: track_id.clone(),
            local_tracks: HashMap::new(),
            router_sender,
            publisher_event_sender: tx,
            close_callback,
            rid_to_ssrc: HashMap::new(),
            publisher_type,
            subscriber_event_sender: vec![],
            relay_sender,
            relayed_targets: vec![],
            relayed_publishers: HashMap::new(),
        };
        let publisher = Arc::new(Mutex::new(publisher));
        {
            let publisher = Arc::clone(&publisher);
            tokio::spawn(async move {
                Publisher::publisher_event_loop(track_id, publisher, rx).await;
            });
        }

        publisher
    }

    pub(crate) fn create_local_track(
        &self,
        track: Arc<TrackRemote>,
        rtp_receiver: Arc<RTCRtpReceiver>,
        rtp_transceiver: Arc<RTCRtpTransceiver>,
        rtcp_sender: Arc<transport::RtcpSender>,
    ) {
        let ssrc = track.ssrc();
        let rid = track.rid().to_string();
        let local_track = LocalTrack::new(
            track,
            rtp_receiver,
            rtp_transceiver,
            rtcp_sender,
            self.publisher_event_sender.clone(),
        );
        let _ = self.publisher_event_sender.send(PublisherEvent::TrackAdded(
            ssrc,
            rid,
            Arc::new(local_track),
        ));
    }

    pub(crate) fn get_local_track(&self, rid: &str) -> Result<Arc<LocalTrack>, Error> {
        if let Some(ssrc) = self.rid_to_ssrc.get(rid) {
            if let Some(track) = self.local_tracks.get(ssrc) {
                tracing::debug!(
                    "Found specified local track with rid={}, ssrc={}",
                    rid,
                    track.ssrc()
                );
                Ok(track.clone())
            } else {
                tracing::debug!("Failed to find track for rid={} and ssrc={}", rid, ssrc);
                self.get_random_local_track()
            }
        } else {
            tracing::debug!("Failed to find ssrc for rid={}", rid);
            self.get_random_local_track()
        }
    }

    fn get_random_local_track(&self) -> Result<Arc<LocalTrack>, Error> {
        let track = self
            .local_tracks
            .values()
            .next()
            .ok_or(Error::new_publisher(
                "Publisher does not have track".to_owned(),
                PublisherErrorKind::TrackNotFoundError,
            ))?;
        Ok(track.clone())
    }

    pub(crate) fn set_subscriber_event_sender(
        &mut self,
        event_sender: mpsc::UnboundedSender<SubscriberEvent>,
    ) {
        self.subscriber_event_sender.push(event_sender);
    }

    pub(crate) async fn set_publisher_type(&mut self, publisher_type: PublisherType) {
        self.publisher_type = publisher_type;
        for sender in self.subscriber_event_sender.iter() {
            if let Err(err) =
                sender.send(SubscriberEvent::SetPrefferedLayer(RID::HIGH.into(), None))
            {
                tracing::error!("Failed to send subscriber event: {}", err);
            }
        }
    }

    pub async fn close(&self) {
        let _ = self.publisher_event_sender.send(PublisherEvent::Close);
    }

    pub(crate) async fn publisher_event_loop(
        id: String,
        publisher: Arc<Mutex<Publisher>>,
        mut event_receiver: mpsc::UnboundedReceiver<PublisherEvent>,
    ) {
        while let Some(event) = event_receiver.recv().await {
            match event {
                PublisherEvent::TrackAdded(ssrc, rid, local_track) => {
                    let mut p = publisher.lock().await;
                    p.local_tracks.insert(ssrc, local_track.clone());
                    p.rid_to_ssrc.insert(rid, ssrc);

                    for (ip, port, router_id) in p.relayed_targets.clone().into_iter() {
                        let relay_sender = p.relay_sender.clone();
                        let track_id = local_track.id();
                        match relay_sender
                            .create_relay_track(
                                ip.clone(),
                                port.clone(),
                                router_id.clone(),
                                track_id.clone(),
                                ssrc,
                                local_track.capability(),
                                local_track.stream_id(),
                                local_track.mime_type(),
                                local_track.rid(),
                                p.publisher_type.clone(),
                            )
                            .await
                        {
                            Ok(udp_port) => {
                                p.relayed_publishers.insert(ssrc, ip.clone());
                                let rtp_packet_sender = local_track.rtp_packet_sender();
                                let event_sender = p.publisher_event_sender.clone();
                                tokio::spawn(async move {
                                    let _ = relay_sender
                                        .rtp_sender_loop(
                                            ip,
                                            udp_port,
                                            ssrc,
                                            track_id,
                                            rtp_packet_sender,
                                        )
                                        .await;
                                    let _ = event_sender
                                        .send(PublisherEvent::RTPSenderLoopClosed(ssrc));
                                });
                            }
                            Err(err) => {
                                tracing::error!(
                                    "Failed to create relay track: track_id={}, ssrc={}: {}",
                                    track_id,
                                    ssrc,
                                    err
                                );
                            }
                        }
                    }
                }
                PublisherEvent::TrackRemoved(ssrc) => {
                    let mut p = publisher.lock().await;
                    p.local_tracks.remove(&ssrc);
                    if p.local_tracks.is_empty() {
                        let _ = p
                            .router_sender
                            .send(RouterEvent::PublisherRemoved(id.clone()));
                        (p.close_callback)(id.clone());
                        let _ = p.publisher_event_sender.send(PublisherEvent::Close);
                    }
                }
                PublisherEvent::RTPSenderLoopClosed(ssrc) => {
                    let mut p = publisher.lock().await;
                    p.relayed_publishers.remove(&ssrc);
                }
                PublisherEvent::Close => {
                    let p = publisher.lock().await;
                    for (_ssrc, track) in &p.local_tracks {
                        track.close();
                    }
                    for (ip, port, router_id) in p.relayed_targets.iter() {
                        if let Err(err) = p
                            .relay_sender
                            .remove_relayed_publisher(
                                ip.to_string(),
                                port.clone(),
                                router_id.to_string(),
                                p.track_id.clone(),
                            )
                            .await
                        {
                            tracing::warn!(
                                "Failed to remove relayed publisher track_id={}: {}",
                                p.track_id,
                                err
                            );
                        }
                    }

                    break;
                }
            }
        }
        tracing::debug!("Publisher {} event loop finished", id);
    }

    /// Forwards the publisher to a specific router in a specific server specified by `ip`. A [`crate::relay::relayed_publisher::RelayedPublisher`] and [`crate::relay::relayed_track::RelayedTrack`] are created in the server after this method.
    /// * `ip` - The IP address of the server to forward the publisher to.
    /// * `port` - The TCP port of the server to forward the publisher to.
    /// * `router_id` - The ID of the router to forward the publisher to.
    pub async fn relay_to(
        &mut self,
        ip: String,
        port: u16,
        router_id: String,
    ) -> Result<bool, Error> {
        for (ssrc, local_track) in self.local_tracks.iter() {
            let udp_port = self
                .relay_sender
                .create_relay_track(
                    ip.clone(),
                    port,
                    router_id.clone(),
                    self.track_id.clone(),
                    local_track.ssrc(),
                    local_track.capability(),
                    local_track.stream_id(),
                    local_track.mime_type(),
                    local_track.rid(),
                    self.publisher_type.clone(),
                )
                .await?;
            let rtp_packet_sender = local_track.rtp_packet_sender();

            if let Some(_) = self.relayed_publishers.get(ssrc).and_then(|saved_ip| {
                if *saved_ip == ip {
                    Some(())
                } else {
                    None
                }
            }) {
                tracing::info!(
                    "Track trac_id={} ssrc={} already relayed to {}:{}",
                    self.track_id,
                    ssrc,
                    ip,
                    udp_port,
                );
            } else {
                let ssrc = ssrc.clone();
                let ip = ip.clone();
                let udp_port = udp_port.clone();
                let track_id = self.track_id.clone();
                let relay_sender = self.relay_sender.clone();
                let event_sender = self.publisher_event_sender.clone();
                tokio::spawn(async move {
                    let _ = relay_sender
                        .rtp_sender_loop(ip, udp_port, ssrc, track_id, rtp_packet_sender)
                        .await;
                    let _ = event_sender.send(PublisherEvent::RTPSenderLoopClosed(ssrc));
                });
            }
            self.relayed_publishers.insert(ssrc.clone(), ip.clone());
        }

        self.relayed_targets.push((ip, port, router_id));
        Ok(true)
    }
}

#[derive(Debug)]
pub(crate) enum PublisherEvent {
    TrackAdded(u32, String, Arc<LocalTrack>),
    TrackRemoved(u32),
    RTPSenderLoopClosed(u32),
    Close,
}

impl Drop for Publisher {
    fn drop(&mut self) {
        tracing::debug!("Publisher track_id={} is dropped", self.track_id);
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize, Default)]
pub enum PublisherType {
    #[default]
    Simple,
    Simulcast,
}

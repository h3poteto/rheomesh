use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use derivative::Derivative;
use rtc::rtp;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};

use crate::{
    config::RID,
    error::{Error, PublisherErrorKind},
    local_track::LocalTrack,
    relay::sender::{RelaySender, RelayUDPSender},
    router::RouterEvent,
    subscriber::SubscriberEvent,
    track::Track,
};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct Publisher {
    pub track_id: String,
    #[derivative(Debug = "ignore")]
    track: Arc<dyn TrackRemote>,
    local_tracks: HashMap<u32, Arc<LocalTrack>>,
    rtp_senders: Arc<RwLock<HashMap<u32, mpsc::UnboundedSender<rtp::packet::Packet>>>>,
    router_sender: mpsc::UnboundedSender<RouterEvent>,
    publisher_event_sender: mpsc::UnboundedSender<PublisherEvent>,
    rid_to_ssrc: HashMap<String, u32>,
    pub publisher_type: PublisherType,
    subscriber_event_sender: Vec<mpsc::UnboundedSender<SubscriberEvent>>,
    relay_sender: Arc<RelaySender>,
    relayed_targets: Vec<(String, u16, String)>,
    relayed_publishers: HashMap<u32, (String, u16)>,
    relay_udp_sender: Option<Arc<RelayUDPSender>>,
    private_ip: String,
    published: bool,
}

impl Publisher {
    pub(crate) async fn new(
        track: Arc<dyn TrackRemote>,
        router_sender: mpsc::UnboundedSender<RouterEvent>,
        relay_sender: Arc<RelaySender>,
        private_ip: String,
    ) -> Arc<Mutex<Publisher>> {
        let (tx, rx) = mpsc::unbounded_channel::<PublisherEvent>();
        let track_id = track.track_id().await;

        let rtp_senders = Arc::new(RwLock::new(HashMap::<
            u32,
            mpsc::UnboundedSender<rtp::packet::Packet>,
        >::new()));

        let publisher = Self {
            track_id: track_id.clone(),
            track: track.clone(),
            local_tracks: HashMap::new(),
            rtp_senders: rtp_senders.clone(),
            router_sender,
            publisher_event_sender: tx.clone(),
            rid_to_ssrc: HashMap::new(),
            publisher_type: PublisherType::Simple,
            subscriber_event_sender: vec![],
            relay_sender,
            relayed_targets: vec![],
            relayed_publishers: HashMap::new(),
            relay_udp_sender: None,
            private_ip,
            published: false,
        };
        let publisher = Arc::new(Mutex::new(publisher));
        {
            let publisher = Arc::clone(&publisher);
            tokio::spawn(async move {
                Publisher::publisher_event_loop(track_id, publisher, rx).await;
            });
        }

        {
            tokio::spawn(async move {
                Self::track_remote_poll(track, rtp_senders, tx).await;
            });
        }

        publisher
    }

    async fn create_local_track(&mut self, ssrc: u32, rid: String) {
        if self.local_tracks.contains_key(&ssrc) {
            return;
        }

        let local_track = Arc::new(
            LocalTrack::new(
                self.track_id.clone(),
                ssrc,
                rid.clone(),
                self.track.clone(),
                self.publisher_event_sender.clone(),
            )
            .await,
        );

        self.rtp_senders
            .write()
            .unwrap()
            .insert(ssrc, local_track.rtp_input_sender());

        self.local_tracks.insert(ssrc, local_track.clone());
        self.rid_to_ssrc.insert(rid, ssrc);

        let _ = self
            .publisher_event_sender
            .send(PublisherEvent::TrackAdded(ssrc, local_track));
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

    pub(crate) async fn track_remote_poll(
        track: Arc<dyn TrackRemote>,
        rtp_senders: Arc<RwLock<HashMap<u32, mpsc::UnboundedSender<rtp::packet::Packet>>>>,
        publisher_event_sender: mpsc::UnboundedSender<PublisherEvent>,
    ) {
        while let Some(event) = track.poll().await {
            match event {
                TrackRemoteEvent::OnOpen(init) => {
                    let ssrc = init.ssrc;
                    let rid = init.rid.unwrap_or_default();
                    tracing::info!("Simulcast track opened with: ssrc={}, rid={}", ssrc, rid);
                    let _ = publisher_event_sender.send(PublisherEvent::TrackOpened(ssrc, rid));
                }
                TrackRemoteEvent::OnRtpPacket(packet) => {
                    let ssrc = packet.header.ssrc;
                    let sender = rtp_senders.read().unwrap().get(&ssrc).cloned();
                    match sender {
                        Some(s) => {
                            let _ = s.send(packet);
                        }
                        None => {
                            tracing::trace!(
                                "No local track for ssrc={}, dropping packet",
                                packet.header.ssrc
                            );
                        }
                    }
                }
                TrackRemoteEvent::OnEnded => break,
                _ => {}
            }
        }
    }

    pub(crate) async fn publisher_event_loop(
        id: String,
        publisher: Arc<Mutex<Publisher>>,
        mut event_receiver: mpsc::UnboundedReceiver<PublisherEvent>,
    ) {
        while let Some(event) = event_receiver.recv().await {
            match event {
                PublisherEvent::TrackOpened(ssrc, rid) => {
                    let mut p = publisher.lock().await;
                    if !rid.is_empty() && p.publisher_type != PublisherType::Simulcast {
                        p.set_publisher_type(PublisherType::Simulcast).await;
                    }
                    p.create_local_track(ssrc, rid).await;
                }
                PublisherEvent::TrackAdded(ssrc, local_track) => {
                    let mut p = publisher.lock().await;

                    if !p.published {
                        p.published = true;
                        let _ = p.router_sender.send(RouterEvent::MediaPublished(
                            p.track_id.clone(),
                            Arc::clone(&publisher),
                        ));
                    }

                    if p.relayed_targets.is_empty() {
                        continue;
                    }

                    if p.relay_udp_sender.is_none() {
                        p.relay_udp_sender = Some(Arc::new(RelayUDPSender::new().await.unwrap()));
                    }
                    let sender_udp_port = p.relay_udp_sender.as_ref().unwrap().port.clone();
                    let private_ip = p.private_ip.clone();

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
                                local_track.parameters(),
                                local_track.stream_id(),
                                local_track.mime_type(),
                                local_track.rid(),
                                p.publisher_type.clone(),
                                sender_udp_port.clone(),
                                private_ip.clone(),
                            )
                            .await
                        {
                            Ok(udp_port) => {
                                p.relayed_publishers
                                    .insert(ssrc, (ip.clone(), udp_port.clone()));
                                let rtp_packet_sender = local_track.rtp_packet_sender();
                                let event_sender = p.publisher_event_sender.clone();
                                let relay_udp_sender = p.relay_udp_sender.clone().unwrap();
                                tokio::spawn(async move {
                                    let _ = relay_udp_sender
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
                    let _ = p
                        .router_sender
                        .send(RouterEvent::PublisherRemoved(p.track_id.clone()));

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
        if self.relay_udp_sender.is_none() {
            self.relay_udp_sender = Some(Arc::new(RelayUDPSender::new().await?));
        }
        let sender_udp_port = self.relay_udp_sender.as_ref().unwrap().port.clone();
        let private_ip = self.private_ip.clone();

        for (ssrc, local_track) in self.local_tracks.iter() {
            let udp_port = self
                .relay_sender
                .create_relay_track(
                    ip.clone(),
                    port,
                    router_id.clone(),
                    self.track_id.clone(),
                    local_track.ssrc(),
                    local_track.parameters(),
                    local_track.stream_id(),
                    local_track.mime_type(),
                    local_track.rid(),
                    self.publisher_type.clone(),
                    sender_udp_port.clone(),
                    private_ip.clone(),
                )
                .await?;
            let rtp_packet_sender = local_track.rtp_packet_sender();

            if let Some(_) = self
                .relayed_publishers
                .get(ssrc)
                .and_then(|(saved_ip, saved_port)| {
                    if *saved_ip == ip && *saved_port == udp_port {
                        Some(())
                    } else {
                        None
                    }
                })
            {
                tracing::info!(
                    "Track trac_id={} ssrc={} already relayed to {}:{}",
                    self.track_id,
                    ssrc,
                    ip,
                    udp_port,
                );
            } else {
                {
                    let ssrc = ssrc.clone();
                    let ip = ip.clone();
                    let udp_port = udp_port.clone();
                    let track_id = self.track_id.clone();
                    let relay_udp_sender = self.relay_udp_sender.clone().unwrap();
                    let event_sender = self.publisher_event_sender.clone();
                    tokio::spawn(async move {
                        let _ = relay_udp_sender
                            .rtp_sender_loop(ip, udp_port, ssrc, track_id, rtp_packet_sender)
                            .await;
                        let _ = event_sender.send(PublisherEvent::RTPSenderLoopClosed(ssrc));
                    });
                }
                {
                    let relay_udp_sender = self.relay_udp_sender.clone().unwrap();
                    let rtcp_sender = local_track.rtcp_sender();
                    tokio::spawn(async move {
                        relay_udp_sender.rtcp_receiver_loop(rtcp_sender).await;
                    });
                }
            }
            self.relayed_publishers
                .insert(ssrc.clone(), (ip.clone(), udp_port));
        }

        self.relayed_targets.push((ip, port, router_id));
        Ok(true)
    }
}

#[derive(Debug)]
pub(crate) enum PublisherEvent {
    TrackOpened(u32, String),
    TrackAdded(u32, Arc<LocalTrack>),
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

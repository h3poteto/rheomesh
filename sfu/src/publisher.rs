use std::{collections::HashMap, sync::Arc};

use derivative::Derivative;
use tokio::sync::{mpsc, Mutex};
use webrtc::{
    rtp_transceiver::{rtp_receiver::RTCRtpReceiver, RTCRtpTransceiver},
    track::track_remote::TrackRemote,
};

use crate::{
    config::RID,
    error::{Error, PublisherErrorKind},
    local_track::LocalTrack,
    router::RouterEvent,
    subscriber::SubscriberEvent,
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
}

impl Publisher {
    pub(crate) fn new(
        track_id: String,
        router_sender: mpsc::UnboundedSender<RouterEvent>,
        publisher_type: PublisherType,
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
                    track.ssrc
                );
                Ok(track.clone())
            } else {
                tracing::debug!("Failed to find track for rid={} and ssrc={}", rid, ssrc);
                self.get_random_local_track()
            }
        } else {
            tracing::debug!("Faild to find ssrc for rid={}", rid);
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

    pub(crate) fn set_publisher_type(&mut self, publisher_type: PublisherType) {
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
                    p.local_tracks.insert(ssrc, local_track);
                    p.rid_to_ssrc.insert(rid, ssrc);
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
                PublisherEvent::Close => {
                    let p = publisher.lock().await;
                    for (_ssrc, track) in &p.local_tracks {
                        track.close().await;
                    }
                    break;
                }
            }
        }
        tracing::debug!("Publisher {} event loop finished", id);
    }
}

#[derive(Debug)]
pub(crate) enum PublisherEvent {
    TrackAdded(u32, String, Arc<LocalTrack>),
    TrackRemoved(u32),
    Close,
}

impl Drop for Publisher {
    fn drop(&mut self) {
        tracing::debug!("Publisher track_id={} is dropped", self.track_id);
    }
}

#[derive(Debug, PartialEq)]
pub enum PublisherType {
    Simple,
    Simulcast,
}

use std::{collections::HashMap, sync::Arc};

use derivative::Derivative;
use tokio::sync::{mpsc, RwLock};
use webrtc::{
    rtp_transceiver::{rtp_receiver::RTCRtpReceiver, RTCRtpTransceiver},
    track::track_remote::TrackRemote,
};

use crate::{
    error::{Error, PublisherErrorKind},
    local_track::LocalTrack,
    router::RouterEvent,
    transport,
};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct Publisher {
    pub track_id: String,
    local_tracks: HashMap<u32, LocalTrack>,
    router_sender: mpsc::UnboundedSender<RouterEvent>,
    publisher_event_sender: mpsc::UnboundedSender<PublisherEvent>,
    #[derivative(Debug = "ignore")]
    close_callback: Box<dyn Fn(String) + Send + Sync>,
}

impl Publisher {
    pub(crate) fn new(
        track_id: String,
        router_sender: mpsc::UnboundedSender<RouterEvent>,
        close_callback: Box<dyn Fn(String) + Send + Sync>,
    ) -> Arc<RwLock<Publisher>> {
        let (tx, rx) = mpsc::unbounded_channel::<PublisherEvent>();

        let publisher = Self {
            track_id: track_id.clone(),
            local_tracks: HashMap::new(),
            router_sender,
            publisher_event_sender: tx,
            close_callback,
        };
        let publisher = Arc::new(RwLock::new(publisher));
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
        let local_track = LocalTrack::new(
            track,
            rtp_receiver,
            rtp_transceiver,
            rtcp_sender,
            self.publisher_event_sender.clone(),
        );
        let _ = self
            .publisher_event_sender
            .send(PublisherEvent::TrackAdded(ssrc, local_track));
    }

    // TODO: Get appropriate local track according to preffered layer
    pub(crate) fn get_local_track(&self) -> Result<&LocalTrack, Error> {
        self.local_tracks
            .values()
            .next()
            .ok_or(Error::new_publisher(
                "Publisher does not have track".to_owned(),
                PublisherErrorKind::TrackNotFoundError,
            ))
    }

    pub async fn close(&self) {
        let _ = self.publisher_event_sender.send(PublisherEvent::Close);
    }

    pub(crate) async fn publisher_event_loop(
        id: String,
        publisher: Arc<RwLock<Publisher>>,
        mut event_receiver: mpsc::UnboundedReceiver<PublisherEvent>,
    ) {
        while let Some(event) = event_receiver.recv().await {
            match event {
                PublisherEvent::TrackAdded(ssrc, local_track) => {
                    let mut p = publisher.write().await;
                    p.local_tracks.insert(ssrc, local_track);
                }
                PublisherEvent::TrackRemoved(ssrc) => {
                    let mut p = publisher.write().await;
                    p.local_tracks.remove(&ssrc);
                    if p.local_tracks.is_empty() {
                        let _ = p
                            .router_sender
                            .send(RouterEvent::PublisherRemoved(id.clone()));
                        (p.close_callback)(id.clone());
                    }
                }
                PublisherEvent::Close => {
                    let p = publisher.read().await;
                    for (_ssrc, track) in &p.local_tracks {
                        track.close().await;
                    }
                }
            }
        }
        tracing::debug!("Publisher {} event loop finished", id);
    }
}

#[derive(Debug)]
pub(crate) enum PublisherEvent {
    TrackAdded(u32, LocalTrack),
    TrackRemoved(u32),
    Close,
}

impl Drop for Publisher {
    fn drop(&mut self) {
        tracing::debug!("Publisher track_id={} is dropped", self.track_id);
    }
}

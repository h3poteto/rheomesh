use std::{collections::HashMap, sync::Arc};

use tokio::sync::{mpsc, Mutex};
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;

use crate::{
    error::{Error, PublisherErrorKind},
    publisher::PublisherType,
    track::Track,
};

use super::relayed_track::RelayedTrack;

/// Publisher that is received from another server.
#[derive(Debug)]
pub struct RelayedPublisher {
    /// The original track ID.
    pub track_id: String,
    pub(crate) local_tracks: HashMap<u32, Arc<RelayedTrack>>,
    publisher_event_sender: mpsc::UnboundedSender<RelayedPublisherEvent>,
    rid_to_ssrc: HashMap<String, u32>,
    pub publisher_type: PublisherType,
}

impl RelayedPublisher {
    pub(crate) fn new(
        track_id: String,
        publisher_type: PublisherType,
    ) -> Arc<Mutex<RelayedPublisher>> {
        let (tx, rx) = mpsc::unbounded_channel::<RelayedPublisherEvent>();

        let publisher = Self {
            track_id: track_id.clone(),
            local_tracks: HashMap::new(),
            publisher_event_sender: tx,
            rid_to_ssrc: HashMap::new(),
            publisher_type,
        };

        let publisher = Arc::new(Mutex::new(publisher));

        {
            let publisher = publisher.clone();
            tokio::spawn(async move {
                Self::publisher_event_loop(track_id, publisher, rx).await;
            });
        }

        publisher
    }

    pub(crate) fn create_relayed_track(
        &self,
        track_id: String,
        ssrc: u32,
        rid: String,
        mime_type: String,
        codec_capability: RTCRtpCodecCapability,
        stream_id: String,
    ) {
        if let None = self.local_tracks.get(&ssrc) {
            let local_track = RelayedTrack::new(
                track_id,
                ssrc,
                rid.clone(),
                mime_type,
                codec_capability,
                stream_id,
            );
            let _ = self
                .publisher_event_sender
                .send(RelayedPublisherEvent::TrackAdded(
                    ssrc,
                    rid,
                    Arc::new(local_track),
                ));
        }
    }

    pub(crate) fn get_relayed_track(&self, rid: &str) -> Result<Arc<RelayedTrack>, Error> {
        if let Some(ssrc) = self.rid_to_ssrc.get(rid) {
            if let Some(track) = self.local_tracks.get(ssrc) {
                tracing::debug!(
                    "Found specified relayed track with rid={}, ssrc={}",
                    rid,
                    track.ssrc()
                );
                Ok(track.clone())
            } else {
                tracing::debug!(
                    "Failed to find relayed track for rid={} and ssrc={}",
                    rid,
                    ssrc
                );
                self.get_random_relayed_track()
            }
        } else {
            tracing::debug!("Failed to find ssrc for rid={}", rid);
            self.get_random_relayed_track()
        }
    }

    fn get_random_relayed_track(&self) -> Result<Arc<RelayedTrack>, Error> {
        let track = self
            .local_tracks
            .values()
            .next()
            .ok_or(Error::new_publisher(
                "RelayedPublisher does not have track".to_owned(),
                PublisherErrorKind::TrackNotFoundError,
            ))?;
        Ok(track.clone())
    }

    pub(crate) async fn publisher_event_loop(
        id: String,
        publisher: Arc<Mutex<RelayedPublisher>>,
        mut event_receiver: mpsc::UnboundedReceiver<RelayedPublisherEvent>,
    ) {
        while let Some(event) = event_receiver.recv().await {
            match event {
                RelayedPublisherEvent::TrackAdded(ssrc, rid, local_track) => {
                    let mut p = publisher.lock().await;
                    p.local_tracks.insert(ssrc, local_track);
                    p.rid_to_ssrc.insert(rid, ssrc);
                }
                RelayedPublisherEvent::Close => {
                    let mut p = publisher.lock().await;
                    p.local_tracks = HashMap::new();
                    p.rid_to_ssrc = HashMap::new();
                    break;
                }
            }
        }
        tracing::debug!("RelayedPublisher {} event loop finished", id);
    }

    pub fn close(&self) {
        let _ = self
            .publisher_event_sender
            .send(RelayedPublisherEvent::Close);
    }
}

#[derive(Debug)]
pub(crate) enum RelayedPublisherEvent {
    TrackAdded(u32, String, Arc<RelayedTrack>),
    Close,
}

impl Drop for RelayedPublisher {
    fn drop(&mut self) {
        tracing::debug!("RelayedPublisher track_id={} is dropped", self.track_id);
    }
}

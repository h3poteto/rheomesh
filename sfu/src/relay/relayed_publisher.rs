use std::{collections::HashMap, sync::Arc};

use tokio::sync::{mpsc, Mutex};
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;

use super::relayed_track::RelayedTrack;

#[derive(Debug)]
pub struct RelayedPublisher {
    pub track_id: String,
    pub(crate) local_tracks: HashMap<u32, Arc<RelayedTrack>>,
    publisher_event_sender: mpsc::UnboundedSender<RelayedPublisherEvent>,
    rid_to_ssrc: HashMap<String, u32>,
}

impl RelayedPublisher {
    pub(crate) fn new(track_id: String) -> Arc<Mutex<RelayedPublisher>> {
        let (tx, rx) = mpsc::unbounded_channel::<RelayedPublisherEvent>();

        let publisher = Self {
            track_id: track_id.clone(),
            local_tracks: HashMap::new(),
            publisher_event_sender: tx,
            rid_to_ssrc: HashMap::new(),
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

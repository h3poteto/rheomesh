use std::sync::Arc;

use derivative::Derivative;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    config::RID,
    error::Error,
    router::{Router, RouterEvent},
    track::Track,
};

use super::recording_track::RecordingTrack;

#[derive(Derivative)]
#[derivative(Debug)]
pub struct RecordingTransport {
    id: String,
    ip_address: String,
    port: u16,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    recording_track: Arc<RecordingTrack>,
}

impl RecordingTransport {
    pub(crate) async fn new(
        ip_address: String,
        port: u16,
        router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    ) -> Result<Self, Error> {
        let id = Uuid::new_v4().to_string();
        let recording_track = RecordingTrack::new().await?;
        let recording_track = Arc::new(recording_track);
        tracing::debug!("Creating RecordingTransport with id: {}", id);

        Ok(Self {
            id,
            router_event_sender,
            ip_address,
            port,
            recording_track,
        })
    }

    pub async fn start_recording(
        &self,
        publisher_id: String,
    ) -> Result<Arc<RecordingTrack>, Error> {
        let local_track = self.find_local_track(publisher_id, RID::HIGH).await?;

        let ip = self.ip_address.clone();
        let port = self.port;
        let track_id = local_track.id();
        let rtp_packet_sender = local_track.rtp_packet_sender();
        let recording_track = self.recording_track.clone();
        tokio::spawn(async move {
            recording_track
                .rtp_sender_loop(ip, port, track_id, rtp_packet_sender)
                .await;
        });

        Ok(self.recording_track.clone())
    }

    async fn find_local_track(
        &self,
        publisher_id: String,
        rid: RID,
    ) -> Result<Arc<dyn Track>, Error> {
        match Router::find_local_track(
            self.router_event_sender.clone(),
            publisher_id.clone(),
            rid.clone(),
        )
        .await
        {
            Ok(track) => Ok(track),
            Err(_) => {
                match Router::find_relayed_track(
                    self.router_event_sender.clone(),
                    publisher_id,
                    rid,
                )
                .await
                {
                    Ok(relayed_track) => Ok(relayed_track),
                    Err(err) => Err(err),
                }
            }
        }
    }

    pub fn close(&self) {
        self.recording_track.close();
    }
}

impl Drop for RecordingTransport {
    fn drop(&mut self) {
        self.recording_track.close();
        tracing::debug!("RecordingTransport {} is dropped", self.id);
    }
}

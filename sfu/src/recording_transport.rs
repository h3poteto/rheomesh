use std::sync::Arc;

use derivative::Derivative;
use tokio::sync::mpsc;

use crate::{
    config::RID,
    error::Error,
    local_track::LocalTrack,
    router::{Router, RouterEvent},
    track::Track,
};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct RecordingTransport {
    ip_address: String,
    port: u16,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
}

impl RecordingTransport {
    pub(crate) fn new(
        ip_address: String,
        port: u16,
        router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    ) -> Self {
        RecordingTransport {
            router_event_sender,
            ip_address,
            port,
        }
    }

    pub async fn start_recording(&self, publisher_id: String) -> Result<(), Error> {
        let local_track = self.find_local_track(publisher_id, RID::HIGH).await?;

        Ok(())
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
}

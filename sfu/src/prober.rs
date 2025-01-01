use std::{sync::Arc, time::Duration};

use tokio::time::sleep;
use uuid::Uuid;
use webrtc::{
    media::Sample, track::track_local::track_local_static_sample::TrackLocalStaticSample,
};

use crate::error::Error;

pub(crate) struct Prober {
    pub _id: String,
}

impl Prober {
    pub(crate) fn new(track: Arc<TrackLocalStaticSample>) -> Self {
        let id = Uuid::new_v4().to_string();

        tokio::spawn(async move {
            let _ = Self::write_rtp(track).await;
        });

        Self { _id: id }
    }

    pub(crate) async fn write_rtp(track: Arc<TrackLocalStaticSample>) -> Result<(), Error> {
        tracing::debug!("Starting prober rtp packets");

        let silent_audio = vec![0u8; 960];
        let silent_audio_bytes = bytes::Bytes::from(silent_audio);
        let duration = Duration::from_millis(20);
        for _ in 0..1500 {
            let sample = Sample {
                data: silent_audio_bytes.clone(),
                duration,
                ..Default::default()
            };
            if let Err(err) = track.write_sample(&sample).await {
                eprintln!("Error sending silent audio frame: {}", err);
                break;
            }
            sleep(duration).await;
        }

        tracing::debug!("Finished sending prober rtp packets");
        Ok(())
    }
}

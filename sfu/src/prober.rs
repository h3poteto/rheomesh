use std::{sync::Arc, time::Duration};

use rtc::{
    media::Sample, peer_connection::configuration::media_engine::MIME_TYPE_OPUS,
    rtp_transceiver::PayloadType,
};
use tokio::time::sleep;
use uuid::Uuid;
use webrtc::{
    media_stream::track_local::static_sample::TrackLocalStaticSample, rtp_transceiver::RtpSender,
};

use crate::error::Error;

pub(crate) struct Prober {
    pub _id: String,
}

impl Prober {
    pub(crate) fn new(
        track: Arc<TrackLocalStaticSample>,
        rtp_sender: Arc<dyn RtpSender>,
        ssrc: u32,
    ) -> Self {
        let id = Uuid::new_v4().to_string();

        tokio::spawn(async move {
            let _ = Self::write_rtp(track, rtp_sender, ssrc).await;
        });

        Self { _id: id }
    }

    pub(crate) async fn write_rtp(
        track: Arc<TrackLocalStaticSample>,
        rtp_sender: Arc<dyn RtpSender>,
        ssrc: u32,
    ) -> Result<(), Error> {
        tracing::debug!("Starting prober rtp packets");

        let silent_audio = vec![0u8; 960];
        let silent_audio_bytes = bytes::Bytes::from(silent_audio);
        let duration = Duration::from_millis(20);
        let mut payload_type: Option<PayloadType> = None;

        for _ in 0..1500 {
            sleep(duration).await;

            if payload_type.is_none() {
                payload_type = find_payload_type(&rtp_sender).await;
            }
            let Some(pt) = payload_type else {
                continue;
            };
            let sample = Sample {
                data: silent_audio_bytes.clone(),
                duration,
                ..Default::default()
            };
            if let Err(err) = track.write_sample(ssrc, pt, &sample, &[]).await {
                tracing::trace!("Error sending silent audio frame: {}", err);
            }
        }

        tracing::debug!("Finished sending prober rtp packets");
        Ok(())
    }
}

async fn find_payload_type(rtp_sender: &Arc<dyn RtpSender>) -> Option<PayloadType> {
    let parameters = rtp_sender.get_parameters().await.ok()?;
    parameters
        .rtp_parameters
        .codecs
        .iter()
        .find(|codec| {
            codec
                .rtp_codec
                .mime_type
                .eq_ignore_ascii_case(MIME_TYPE_OPUS)
        })
        .map(|codec| codec.payload_type)
}

use std::sync::Arc;

use derivative::Derivative;
use tokio::sync::mpsc;
use uuid::Uuid;
use webrtc::sdp::{
    self,
    description::{
        common::{Address, Attribute, ConnectionInformation},
        media::MediaName,
    },
    MediaDescription, SessionDescription,
};

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

    pub async fn generate_sdp(&self, publisher_id: String) -> Result<String, Error> {
        let local_track = self.find_local_track(publisher_id, RID::HIGH).await?;
        let media_description = MediaDescription {
            media_name: MediaName {
                media: "video".to_string(),
                port: sdp::description::media::RangedPort {
                    value: self.port as isize,
                    range: None,
                },
                protos: vec!["RTP/AVP".to_string()],
                formats: vec![local_track.payload_type().to_string()],
            },
            media_title: None,
            connection_information: None,
            bandwidth: vec![],
            encryption_key: None,
            attributes: vec![
                Attribute::new(
                    "rtpmap".to_string(),
                    Some(Self::rtpmap(local_track.clone())),
                ),
                Attribute::new("sendonly".to_string(), None),
            ],
        };
        let session_description = SessionDescription {
            version: 0,
            origin: sdp::description::session::Origin {
                username: "-".to_string(),
                session_id: 0,
                session_version: 0,
                network_type: "IN".to_string(),
                address_type: "IP4".to_string(),
                unicast_address: self.ip_address.clone(),
            },
            session_name: "Recording Transport".to_string(),
            session_information: None,
            uri: None,
            email_address: None,
            phone_number: None,
            connection_information: Some(ConnectionInformation {
                network_type: "IN".to_string(),
                address_type: "IP4".to_string(),
                address: Some(Address {
                    address: self.ip_address.clone(),
                    ttl: None,
                    range: None,
                }),
            }),
            bandwidth: vec![],
            time_descriptions: vec![sdp::description::session::TimeDescription {
                timing: sdp::description::session::Timing {
                    start_time: 0,
                    stop_time: 0,
                },
                repeat_times: vec![],
            }],
            time_zones: vec![],
            encryption_key: None,
            attributes: vec![],
            media_descriptions: vec![media_description],
        };

        let sdp = session_description.marshal();
        Ok(sdp)
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
        tracing::debug!(
            "find_local_track called for publisher_id: {}, rid: {:?}",
            publisher_id,
            rid
        );
        match Router::find_local_track(
            self.router_event_sender.clone(),
            publisher_id.clone(),
            rid.clone(),
        )
        .await
        {
            Ok(track) => {
                tracing::debug!("found");
                Ok(track)
            }
            Err(err) => {
                tracing::warn!("find_local_track error: {}", err);
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

    fn rtpmap(local_track: Arc<dyn Track>) -> String {
        let mime_type = local_track.capability().mime_type.clone();
        let codec = Self::extract_codec(mime_type.as_str());
        if local_track.capability().channels > 0 {
            format!(
                "{} {}/{}/{}",
                local_track.payload_type(),
                codec,
                local_track.capability().clock_rate,
                local_track.capability().channels,
            )
        } else {
            format!(
                "{} {}/{}",
                local_track.payload_type(),
                codec,
                local_track.capability().clock_rate
            )
        }
    }

    fn extract_codec(mime_type: &str) -> &str {
        if let Some(slash_pos) = mime_type.find('/') {
            &mime_type[slash_pos + 1..]
        } else {
            mime_type
        }
    }
}

impl Drop for RecordingTransport {
    fn drop(&mut self) {
        self.recording_track.close();
        tracing::debug!("RecordingTransport {} is dropped", self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_codec() {
        assert_eq!(RecordingTransport::extract_codec("video/VP8"), "VP8");
        assert_eq!(RecordingTransport::extract_codec("video/VP9"), "VP9");
        assert_eq!(RecordingTransport::extract_codec("audio/opus"), "opus");
        assert_eq!(RecordingTransport::extract_codec("audio/mp3"), "mp3");
    }
}

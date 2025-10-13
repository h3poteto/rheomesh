use std::{collections::HashMap, sync::Arc};

use tokio::{
    net::UdpSocket,
    sync::{Mutex, broadcast, mpsc},
};
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;

use crate::{
    error::{Error, PublisherErrorKind, RelayErrorKind},
    publisher::PublisherType,
    track::Track,
    transport,
    utils::ports::find_unused_port,
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
    rtcp_sender: Arc<transport::RtcpSender>,
    closed_sender: broadcast::Sender<bool>,
}

impl RelayedPublisher {
    pub(crate) async fn new(
        track_id: String,
        publisher_type: PublisherType,
        rtcp_receiver_ip: String,
        rtcp_receiver_port: u16,
    ) -> Result<Arc<Mutex<RelayedPublisher>>, Error> {
        let (tx, rx) = mpsc::unbounded_channel::<RelayedPublisherEvent>();
        let (rtcp_sender, rtcp_receiver) = mpsc::unbounded_channel();
        let (closed_sender, _) = broadcast::channel(1);

        let publisher = Self {
            track_id: track_id.clone(),
            local_tracks: HashMap::new(),
            publisher_event_sender: tx,
            rid_to_ssrc: HashMap::new(),
            publisher_type,
            rtcp_sender: Arc::new(rtcp_sender),
            closed_sender: closed_sender.clone(),
        };

        let publisher = Arc::new(Mutex::new(publisher));

        tracing::debug!("RelayedPublisher {} created", track_id,);

        {
            let publisher = publisher.clone();
            tokio::spawn(async move {
                Self::publisher_event_loop(track_id, publisher, rx).await;
            });
        }

        {
            let sender_port = find_unused_port().ok_or(Error::new_relay(
                "Failed to find unused port for RTCP receiver".to_string(),
                RelayErrorKind::RelayReceiverError,
            ))?;
            let udp_socket = UdpSocket::bind(format!("0.0.0.0:{}", sender_port)).await?;
            let closed_sender = closed_sender.clone();
            tokio::spawn(async move {
                if let Err(err) = Self::rtcp_event_loop(
                    udp_socket,
                    rtcp_receiver,
                    rtcp_receiver_ip,
                    rtcp_receiver_port,
                    closed_sender,
                )
                .await
                {
                    tracing::error!(
                        "Failed to start RTCP event loop for RelayedPublisher: {}",
                        err
                    );
                }
            });
        }

        Ok(publisher)
    }

    pub(crate) fn create_relayed_track(
        &mut self,
        track_id: String,
        ssrc: u32,
        rid: String,
        mime_type: String,
        codec_parameters: RTCRtpCodecParameters,
        stream_id: String,
    ) {
        if let None = self.local_tracks.get(&ssrc) {
            let rtcp_sender = self.rtcp_sender.clone();

            let local_track = RelayedTrack::new(
                track_id.clone(),
                ssrc,
                rid.clone(),
                mime_type,
                codec_parameters,
                stream_id,
                rtcp_sender,
            );
            let _ = self
                .publisher_event_sender
                .send(RelayedPublisherEvent::TrackAdded(
                    ssrc,
                    rid.clone(),
                    Arc::new(local_track),
                ));
            tracing::debug!(
                "RelayedTrack is created with track_id={}, ssrc={}, rid={}",
                track_id,
                ssrc,
                rid
            );
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
                    p.closed_sender.send(true).ok();
                    break;
                }
            }
        }
        tracing::debug!("RelayedPublisher {} event loop finished", id);
    }

    pub(crate) async fn rtcp_event_loop(
        udp_socket: UdpSocket,
        mut rtcp_receiver: transport::RtcpReceiver,
        rtcp_receiver_ip: String,
        rtcp_receiver_port: u16,
        closed_sender: broadcast::Sender<bool>,
    ) -> Result<(), Error> {
        let mut closed_receiver = closed_sender.subscribe();
        drop(closed_sender);

        let addr = format!("{}:{}", rtcp_receiver_ip, rtcp_receiver_port);

        loop {
            tokio::select! {
                _ = closed_receiver.recv() => {
                    tracing::debug!("RTCP receiver closed, exiting loop");
                    break;
                }
                res = rtcp_receiver.recv() => {
                    match res {
                        Some(res) => {
                            match res.marshal() {
                                Ok(buf) => {
                                    if let Err(err) = udp_socket.send_to(&buf, &addr).await {
                                        tracing::error!("Failed to send RTCP packet: {}", err);
                                    }
                                }
                                Err(err) => {
                                    tracing::error!("Failed to marshal RTCP packet: {}", err);
                                    continue;
                                }
                            }

                        }
                        None => {
                        }
                    }

                }
            }
        }

        Ok(())
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
        let _ = self.closed_sender.send(true);
    }
}

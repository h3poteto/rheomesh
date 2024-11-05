use crate::{
    config::{MediaConfig, WebRTCTransportConfig},
    data_publisher::DataPublisher,
    error::{Error, PublisherErrorKind, TransportErrorKind},
    publisher::Publisher,
    router::RouterEvent,
    transport::{OnIceCandidateFn, OnTrackFn, RtcpReceiver, RtcpSender, Transport},
};
use enclose::enc;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};
use uuid::Uuid;
use webrtc::{
    data_channel::RTCDataChannel,
    ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
    peer_connection::{sdp::session_description::RTCSessionDescription, RTCPeerConnection},
    rtp_transceiver::{rtp_receiver::RTCRtpReceiver, RTCRtpTransceiver},
    track::track_remote::TrackRemote,
};

#[derive(Clone)]
pub struct PublishTransport {
    pub id: String,
    peer_connection: Arc<RTCPeerConnection>,
    pending_candidates: Arc<Mutex<Vec<RTCIceCandidateInit>>>,
    published_sender: broadcast::Sender<Arc<Publisher>>,
    published_receiver: Arc<Mutex<broadcast::Receiver<Arc<Publisher>>>>,
    data_published_sender: broadcast::Sender<Arc<DataPublisher>>,
    data_published_receiver: Arc<Mutex<broadcast::Receiver<Arc<DataPublisher>>>>,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    // For RTCP writer
    rtcp_sender_channel: Arc<RtcpSender>,
    rtcp_receiver_channel: Arc<Mutex<RtcpReceiver>>,
    stop_sender_channel: Arc<Mutex<mpsc::UnboundedSender<()>>>,
    stop_receiver_channel: Arc<Mutex<mpsc::UnboundedReceiver<()>>>,
    // For callback fn
    on_ice_candidate_fn: Arc<Mutex<OnIceCandidateFn>>,
    on_track_fn: Arc<Mutex<OnTrackFn>>,
}

impl PublishTransport {
    pub async fn new(
        router_event_sender: mpsc::UnboundedSender<RouterEvent>,
        media_config: MediaConfig,
        transport_config: WebRTCTransportConfig,
    ) -> Self {
        let id = Uuid::new_v4().to_string();
        let (s, r) = mpsc::unbounded_channel();
        let (stop_sender, stop_receiver) = mpsc::unbounded_channel();
        let (published_sender, published_receiver) = broadcast::channel(1024);
        let (data_published_sender, data_published_receiver) = broadcast::channel(1024);

        let peer_connection = Self::generate_peer_connection(media_config, transport_config)
            .await
            .unwrap();

        let mut transport = Self {
            id,
            peer_connection: Arc::new(peer_connection),
            router_event_sender,
            published_sender,
            published_receiver: Arc::new(Mutex::new(published_receiver)),
            data_published_sender,
            data_published_receiver: Arc::new(Mutex::new(data_published_receiver)),
            pending_candidates: Arc::new(Mutex::new(Vec::new())),
            rtcp_sender_channel: Arc::new(s),
            rtcp_receiver_channel: Arc::new(Mutex::new(r)),
            stop_sender_channel: Arc::new(Mutex::new(stop_sender)),
            stop_receiver_channel: Arc::new(Mutex::new(stop_receiver)),
            on_ice_candidate_fn: Arc::new(Mutex::new(Box::new(|_| {}))),
            on_track_fn: Arc::new(Mutex::new(Box::new(|_, _, _| {}))),
        };

        transport.rtcp_writer_loop();
        transport.ice_state_hooks().await;

        tracing::debug!("PublishTransport {} is created", transport.id);

        transport
    }

    pub async fn get_answer(
        &self,
        sdp: RTCSessionDescription,
    ) -> Result<RTCSessionDescription, Error> {
        let answer = self.get_answer_for_offer(sdp).await?;
        Ok(answer)
    }

    pub async fn publish(&self, track_id: String) -> Result<Arc<Publisher>, Error> {
        let receiver = self.published_receiver.clone();
        while let Ok(publisher) = receiver.lock().await.recv().await {
            if publisher.id == track_id {
                return Ok(publisher);
            }
        }
        Err(Error::new_publisher(
            "Failed to get published track".to_string(),
            PublisherErrorKind::TrackNotPublishedError,
        ))
    }

    pub async fn data_publish(&self, channel_id: String) -> Result<Arc<DataPublisher>, Error> {
        let receiver = self.data_published_receiver.clone();
        while let Ok(data_publisher) = receiver.lock().await.recv().await {
            if data_publisher.id == channel_id {
                return Ok(data_publisher);
            }
        }
        Err(Error::new_publisher(
            "Failed to get published data channel".to_owned(),
            PublisherErrorKind::DataChannelNotPublishedError,
        ))
    }

    async fn get_answer_for_offer(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<RTCSessionDescription, Error> {
        tracing::debug!("publisher set remote description");
        self.peer_connection.set_remote_description(offer).await?;
        let pendings = self.pending_candidates.lock().await;
        for candidate in pendings.iter() {
            tracing::debug!("Adding pending ICE candidate: {:#?}", candidate);
            if let Err(err) = self
                .peer_connection
                .add_ice_candidate(candidate.clone())
                .await
            {
                tracing::error!("failed to add_ice_candidate: {}", err);
            }
        }

        let answer = self.peer_connection.create_answer(None).await?;
        self.peer_connection.set_local_description(answer).await?;
        match self.peer_connection.local_description().await {
            Some(answer) => Ok(answer),
            None => Err(Error::new_transport(
                "Failed to set local description".to_string(),
                TransportErrorKind::LocalDescriptionError,
            )),
        }
    }

    fn rtcp_writer_loop(&self) {
        let rtcp_receiver = self.rtcp_receiver_channel.clone();
        let stop_receiver = self.stop_receiver_channel.clone();
        let pc = self.peer_connection.clone();
        tokio::spawn(async move {
            tracing::info!("RTCP writer loop");
            loop {
                let mut rtcp_receiver = rtcp_receiver.lock().await;
                let mut stop_receiver = stop_receiver.lock().await;
                tokio::select! {
                    data = rtcp_receiver.recv() => {
                        if let Some(data) = data {
                            if let Err(err) = pc.write_rtcp(&[data]).await {
                                tracing::error!("Error writing RTCP: {}", err);
                            }
                        }
                    }
                    _data = stop_receiver.recv() => {
                        tracing::info!("RTCP writer loop stopped");
                        return;
                    }
                };
            }
        });
    }

    // ICE events
    async fn ice_state_hooks(&mut self) {
        let peer = self.peer_connection.clone();
        let on_ice_candidate = Arc::clone(&self.on_ice_candidate_fn);

        // This callback is called after initializing PeerConnection with ICE servers.
        peer.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
            Box::pin({
                let func = on_ice_candidate.clone();
                async move {
                    let locked = func.lock().await;
                    if let Some(candidate) = candidate {
                        tracing::info!("on ice candidate: {}", candidate);
                        // Call on_ice_candidate_fn as callback.
                        (locked)(candidate);
                    }
                }
            })
        }));

        peer.on_negotiation_needed(Box::new(move || {
            Box::pin(async move {
                tracing::error!("on negotiation needed in publisher");
            })
        }));

        let on_track = Arc::clone(&self.on_track_fn);
        let router_sender = self.router_event_sender.clone();
        let rtcp_sender = self.rtcp_sender_channel.clone();
        let published_sender = self.published_sender.clone();
        peer.on_track(Box::new(enc!( (on_track, router_sender, rtcp_sender, published_sender)
            move |track: Arc<TrackRemote>,
                  receiver: Arc<RTCRtpReceiver>,
                  transceiver: Arc<RTCRtpTransceiver>| {
                Box::pin(enc!( (on_track, router_sender, rtcp_sender, published_sender) async move {
                    let locked = on_track.lock().await;
                    let id = track.id();
                    let ssrc = track.ssrc();
                    tracing::info!("Track published: id={}, ssrc={}", id, ssrc);

                    let (publisher, closed) = Publisher::new(track.clone(), receiver.clone(), transceiver.clone(), rtcp_sender);

                    published_sender.send(publisher.clone()).expect("could not send published track id to publisher");
                    let _ = router_sender.send(RouterEvent::TrackPublished(publisher));

                    (locked)(track, receiver, transceiver);

                    // Keep this thread until closed, and send TrackRemove event
                    let _ = closed.await;
                    let _ = router_sender.send(RouterEvent::TrackRemoved(id));
                }))
            }
        )));

        peer.on_ice_gathering_state_change(Box::new(move |state| {
            Box::pin(async move {
                tracing::debug!("ICE gathering state changed: {}", state);
            })
        }));

        let router_sender = self.router_event_sender.clone();
        let data_published_sender = self.data_published_sender.clone();
        peer.on_data_channel(Box::new(
            enc!((router_sender, data_published_sender) move |dc: Arc<RTCDataChannel>| {
                Box::pin(enc!((router_sender, data_published_sender) async move {
                    let id = dc.id().to_string();
                    tracing::info!("Data channel created: id={}, label={}", id, dc.label());

                    let (data_publisher, closed) = DataPublisher::new(dc);

                    data_published_sender.send(data_publisher.clone()).expect("could not send data published to publisher");
                    let _ = router_sender.send(RouterEvent::DataPublished(data_publisher));

                    let _ = closed.await;
                    let _ = router_sender.send(RouterEvent::DataRemoved(id));
                }))
            }),
        ));
    }

    // Hooks
    pub async fn on_ice_candidate(&self, f: OnIceCandidateFn) {
        let mut callback = self.on_ice_candidate_fn.lock().await;
        *callback = f;
    }

    pub async fn on_track(&mut self, f: OnTrackFn) {
        let mut callback = self.on_track_fn.lock().await;
        *callback = f;
    }

    pub async fn close(&self) -> Result<(), Error> {
        if let Err(err) = self.stop_sender_channel.lock().await.send(()) {
            tracing::error!("failed to stop rtcp writer loop: {}", err);
        }
        self.peer_connection.close().await?;
        Ok(())
    }
}

impl Transport for PublishTransport {
    async fn add_ice_candidate(&self, candidate: RTCIceCandidateInit) -> Result<(), Error> {
        if let Some(_rd) = self.peer_connection.remote_description().await {
            tracing::debug!("Adding ICE candidate for {:#?}", candidate);
            let _ = self
                .peer_connection
                .add_ice_candidate(candidate.clone())
                .await?;
        } else {
            tracing::debug!("Pending ICE candidate for {:#?}", candidate);
            self.pending_candidates.lock().await.push(candidate.clone());
        }

        Ok(())
    }
}

impl Drop for PublishTransport {
    fn drop(&mut self) {
        tracing::debug!("PublishTransport {} is dropped", self.id);
    }
}

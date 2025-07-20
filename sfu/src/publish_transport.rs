use crate::{
    config::{MediaConfig, WebRTCTransportConfig},
    data_publisher::DataPublisher,
    error::{Error, PublisherErrorKind, TransportErrorKind},
    publisher::{Publisher, PublisherType},
    relay::sender::RelaySender,
    replay_channel,
    router::RouterEvent,
    transport::{OnIceCandidateFn, OnTrackFn, PeerConnection, RtcpReceiver, RtcpSender, Transport},
};
use derivative::Derivative;
use enclose::enc;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::{broadcast, mpsc, Mutex};
use uuid::Uuid;
use webrtc::{
    data_channel::RTCDataChannel,
    ice_transport::{
        ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
        ice_gathering_state::RTCIceGatheringState,
    },
    peer_connection::{
        peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription, signaling_state::RTCSignalingState,
        RTCPeerConnection,
    },
    rtp_transceiver::{rtp_receiver::RTCRtpReceiver, RTCRtpTransceiver},
    stats,
    track::track_remote::TrackRemote,
};

/// This handle [`webrtc::peer_connection::RTCPeerConnection`] methods for publisher.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct PublishTransport {
    pub id: String,
    peer_connection: Arc<RTCPeerConnection>,
    pending_candidates: Arc<Mutex<Vec<RTCIceCandidateInit>>>,
    published_channel: Arc<replay_channel::ReplayChannel<Arc<Mutex<Publisher>>>>,
    published_receiver: Arc<Mutex<mpsc::Receiver<Arc<Mutex<Publisher>>>>>,
    data_published_sender: broadcast::Sender<Arc<DataPublisher>>,
    data_published_receiver: Arc<Mutex<broadcast::Receiver<Arc<DataPublisher>>>>,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    // For RTCP writer
    rtcp_sender_channel: Arc<RtcpSender>,
    rtcp_receiver_channel: Arc<Mutex<RtcpReceiver>>,
    stop_sender_channel: Arc<Mutex<mpsc::UnboundedSender<()>>>,
    stop_receiver_channel: Arc<Mutex<mpsc::UnboundedReceiver<()>>>,
    // For callback fn
    #[derivative(Debug = "ignore")]
    on_ice_candidate_fn: Arc<Mutex<OnIceCandidateFn>>,
    #[derivative(Debug = "ignore")]
    on_track_fn: Arc<Mutex<OnTrackFn>>,
    signaling_pending: Arc<AtomicBool>,
    publishers: Arc<Mutex<HashMap<String, Arc<Mutex<Publisher>>>>>,
    relay_sender: Arc<RelaySender>,
    private_ip: String,
}

impl PublishTransport {
    pub(crate) async fn new(
        router_event_sender: mpsc::UnboundedSender<RouterEvent>,
        media_config: MediaConfig,
        transport_config: WebRTCTransportConfig,
        relay_sender: Arc<RelaySender>,
        private_ip: String,
    ) -> Self {
        let id = Uuid::new_v4().to_string();
        let (s, r) = mpsc::unbounded_channel();
        let (stop_sender, stop_receiver) = mpsc::unbounded_channel();
        let (published_channel, published_receiver) =
            replay_channel::ReplayChannel::<Arc<Mutex<Publisher>>>::new(65535);
        let (data_published_sender, data_published_receiver) = broadcast::channel(1024);

        let peer_connection = Self::generate_peer_connection(media_config, transport_config)
            .await
            .unwrap();

        let mut transport = Self {
            id,
            peer_connection: Arc::new(peer_connection),
            router_event_sender,
            published_channel: Arc::new(published_channel),
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
            signaling_pending: Arc::new(AtomicBool::new(false)),
            publishers: Arc::new(Mutex::new(HashMap::new())),
            relay_sender,
            private_ip,
        };

        transport.rtcp_writer_loop();
        transport.ice_state_hooks().await;

        tracing::debug!("PublishTransport {} is created", transport.id);

        transport
    }

    /// This sets the offer to the [`webrtc::peer_connection::RTCPeerConnection`] and creates answer sdp for it.
    pub async fn get_answer(
        &self,
        sdp: RTCSessionDescription,
    ) -> Result<RTCSessionDescription, Error> {
        let answer = self.get_answer_for_offer(sdp).await?;
        Ok(answer)
    }

    /// This starts publishing the track.
    /// * `publisher_id` - The id of the publisher to be published. You can get it from the publisher object in client-side.
    pub async fn publish(&self, publisher_id: String) -> Result<Arc<Mutex<Publisher>>, Error> {
        for publisher in self.published_channel.subscribe().await {
            #[allow(unused)]
            let mut published_track_id = "".to_owned();
            {
                let p = publisher.lock().await;
                published_track_id = p.track_id.clone();
            }
            if published_track_id == publisher_id {
                return Ok(publisher);
            }
        }

        let receiver = self.published_receiver.clone();
        tracing::debug!("waiting receiver");
        while let Some(publisher) = receiver.lock().await.recv().await {
            tracing::debug!("receive publisher");
            #[allow(unused)]
            let mut published_track_id = "".to_owned();
            {
                let p = publisher.lock().await;
                published_track_id = p.track_id.clone();
            }
            if published_track_id == publisher_id {
                return Ok(publisher);
            }
        }
        Err(Error::new_publisher(
            "Failed to get published track".to_string(),
            PublisherErrorKind::TrackNotPublishedError,
        ))
    }

    /// This starts publishing the data channel.
    /// * `label` - The label of the data channel to be published. You can get it from the data channel object in client-side.
    pub async fn data_publish(&self, label: String) -> Result<Arc<DataPublisher>, Error> {
        let receiver = self.data_published_receiver.clone();
        while let Ok(data_publisher) = receiver.lock().await.recv().await {
            if data_publisher.label == label {
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
        if self.peer_connection.signaling_state() != RTCSignalingState::Stable {
            return Err(Error::new_transport(
                format!(
                    "Signaling state is {}",
                    self.peer_connection.signaling_state()
                ),
                TransportErrorKind::SignalingStateInvalidError,
            ));
        }
        self.signaling_pending.store(true, Ordering::Relaxed);
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
        let published_sender = self.published_channel.clone();
        let publishers = self.publishers.clone();
        let relay_sender = self.relay_sender.clone();
        let private_ip = self.private_ip.clone();
        peer.on_track(Box::new(enc!( (on_track, router_sender, rtcp_sender, published_sender, publishers, relay_sender, private_ip)
            move |track: Arc<TrackRemote>,
                  receiver: Arc<RTCRtpReceiver>,
                  transceiver: Arc<RTCRtpTransceiver>| {
                Box::pin(enc!( (on_track, router_sender, rtcp_sender, published_sender, publishers, relay_sender, private_ip) async move {
                    let id = track.id();
                    let ssrc = track.ssrc();
                    tracing::info!("Track published: track_id={}, ssrc={}, rid={}", id, ssrc, track.rid());
                    tracing::debug!("codec: {:#?}", track.codec());

                    {
                        let publishers_clone = publishers.clone();
                        let mut publishers = publishers.lock().await;
                        if let Some(p) = publishers.get(&id) {
                            let mut publisher = p.lock().await;
                            publisher.set_publisher_type(PublisherType::Simulcast).await;
                            publisher.create_local_track(track.clone(), receiver.clone(), transceiver.clone());
                        } else {
                            let publisher = Publisher::new(id.clone(), router_sender.clone(), PublisherType::Simple, relay_sender, private_ip, rtcp_sender, Box::new(move |closed_id| {
                                let publishers_clone = publishers_clone.clone();
                                tokio::spawn(async move {
                                    let mut guard = publishers_clone.lock().await;
                                    guard.remove(&closed_id);
                                });
                            }));
                            {
                                let publisher = publisher.lock().await;
                                publisher.create_local_track(track.clone(), receiver.clone(), transceiver.clone());
                            }

                            publishers.insert(id.clone(), publisher.clone());
                            published_sender.send(publisher.clone()).await;
                            let _ = router_sender.send(RouterEvent::MediaPublished(id, publisher)).expect("could not send router event");
                        }
                    }

                    let locked = on_track.lock().await;
                    (locked)(track, receiver, transceiver);
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
                    let channel = dc.clone();
                    dc.on_open(Box::new(enc!((channel, router_sender, data_published_sender) move || {
                        let id = channel.id().to_string();
                        tracing::info!("DataChannel is opened: id={}, label={}, readyState={}", id, channel.label(), channel.ready_state());
                        Box::pin(async move {
                            let data_publisher = Arc::new(DataPublisher::new(channel, router_sender.clone()));
                            data_published_sender.send(data_publisher.clone()).expect("could not send data published to publisher");
                            let _ = router_sender.send(RouterEvent::DataPublished(data_publisher));
                        })
                    })));
                }))
            }),
        ));

        let signaling_pending = self.signaling_pending.clone();
        peer.on_signaling_state_change(Box::new(enc!((signaling_pending) move |state| {
            tracing::debug!("Signaling state changed: {}", state);
            if state == RTCSignalingState::Stable {
                signaling_pending.store(false, Ordering::Relaxed);
            }

            Box::pin(async {})
        })));
    }

    // Hooks
    /// Set callback function when the [`webrtc::peer_connection::RTCPeerConnection`] receives `on_ice_candidate` events.
    pub async fn on_ice_candidate(&self, f: OnIceCandidateFn) {
        let mut callback = self.on_ice_candidate_fn.lock().await;
        *callback = f;
    }

    /// Set callback function when the [`webrtc::peer_connection::RTCPeerConnection`] receives `on_track` events.
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

impl PeerConnection for PublishTransport {}

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

    fn signaling_state(&self) -> RTCSignalingState {
        self.peer_connection.signaling_state()
    }

    fn ice_gathering_state(&self) -> RTCIceGatheringState {
        self.peer_connection.ice_gathering_state()
    }

    fn connection_state(&self) -> RTCPeerConnectionState {
        self.peer_connection.connection_state()
    }

    async fn get_stats(&self) -> stats::StatsReport {
        let report = self.peer_connection.get_stats().await;
        report
    }
}

impl Drop for PublishTransport {
    fn drop(&mut self) {
        tracing::debug!("PublishTransport {} is dropped", self.id);
    }
}

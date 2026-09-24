use crate::{
    config::{MediaConfig, WebRTCTransportConfig},
    data_publisher::DataPublisher,
    error::{Error, PublisherErrorKind, TransportErrorKind},
    publisher::Publisher,
    relay::sender::RelaySender,
    replay_channel,
    router::RouterEvent,
    transport::{self, OnIceCandidateFn, OnTrackFn, Transport},
};
use derivative::Derivative;
use rtc::statistics::StatsSelector;
use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use tokio::sync::{Mutex, broadcast, mpsc};
use uuid::Uuid;
use webrtc::{
    data_channel::DataChannel,
    media_stream::track_remote::TrackRemote,
    peer_connection::{
        PeerConnection, PeerConnectionEventHandler, RTCIceCandidateInit, RTCIceGatheringState,
        RTCPeerConnectionIceEvent, RTCPeerConnectionState, RTCSessionDescription,
        RTCSignalingState, RTCStatsReport,
    },
};

/// This handle [`webrtc::peer_connection::RTCPeerConnection`] methods for publisher.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct PublishTransport {
    pub id: String,
    #[derivative(Debug = "ignore")]
    peer_connection: Arc<dyn PeerConnection>,
    pending_candidates: Arc<Mutex<Vec<RTCIceCandidateInit>>>,
    published_channel: Arc<replay_channel::ReplayChannel<Arc<Mutex<Publisher>>>>,
    published_receiver: Arc<Mutex<mpsc::Receiver<Arc<Mutex<Publisher>>>>>,
    data_published_sender: broadcast::Sender<Arc<Mutex<DataPublisher>>>,
    data_published_receiver: Arc<Mutex<broadcast::Receiver<Arc<Mutex<DataPublisher>>>>>,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    // For callback fn
    #[derivative(Debug = "ignore")]
    on_ice_candidate_fn: Arc<Mutex<OnIceCandidateFn>>,
    #[derivative(Debug = "ignore")]
    on_track_fn: Arc<Mutex<OnTrackFn>>,
    signaling_pending: Arc<AtomicBool>,
    relay_sender: Arc<RelaySender>,
    private_ip: String,
    #[derivative(Debug = "ignore")]
    handler: Arc<PublishHandler>,
}

impl PublishTransport {
    pub(crate) async fn new(
        router_event_sender: mpsc::UnboundedSender<RouterEvent>,
        media_config: MediaConfig,
        transport_config: WebRTCTransportConfig,
        relay_sender: Arc<RelaySender>,
        private_ip: String,
    ) -> Result<Self, Error> {
        let id = Uuid::new_v4().to_string();
        let (published_channel, published_receiver) =
            replay_channel::ReplayChannel::<Arc<Mutex<Publisher>>>::new(65535);
        let (data_published_sender, data_published_receiver) = broadcast::channel(1024);

        let on_ice_candidate_fn: Arc<Mutex<OnIceCandidateFn>> =
            Arc::new(Mutex::new(Box::new(|_| {})));
        let on_track_fn: Arc<Mutex<OnTrackFn>> = Arc::new(Mutex::new(Box::new(|_| {})));
        let signaling_pending = Arc::new(AtomicBool::new(false));
        let published_channel = Arc::new(published_channel);
        let published_receiver = Arc::new(Mutex::new(published_receiver));

        let handler = Arc::new(PublishHandler {
            router_event_sender: router_event_sender.clone(),
            on_ice_candidate_fn: on_ice_candidate_fn.clone(),
            on_track_fn: on_track_fn.clone(),
            signaling_pending: signaling_pending.clone(),
            published_channel: published_channel.clone(),
            published_receiver: published_receiver.clone(),
            data_published_sender: data_published_sender.clone(),
            relay_sender: relay_sender.clone(),
            private_ip: private_ip.clone(),
            signaling_state: Arc::new(RwLock::new(RTCSignalingState::default())),
            ice_gathering_state: Arc::new(RwLock::new(RTCIceGatheringState::default())),
            connection_state: Arc::new(RwLock::new(RTCPeerConnectionState::default())),
        });

        let peer_connection =
            transport::generate_peer_connection(handler.clone(), media_config, transport_config)
                .await?;

        let transport = Self {
            id,
            peer_connection,
            router_event_sender,
            published_channel,
            published_receiver,
            data_published_sender,
            data_published_receiver: Arc::new(Mutex::new(data_published_receiver)),
            pending_candidates: Arc::new(Mutex::new(Vec::new())),
            on_ice_candidate_fn,
            on_track_fn,
            signaling_pending,
            relay_sender,
            private_ip,
            handler,
        };

        tracing::debug!("PublishTransport {} is created", transport.id);

        Ok(transport)
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
    pub async fn data_publish(&self, label: String) -> Result<Arc<Mutex<DataPublisher>>, Error> {
        let receiver = self.data_published_receiver.clone();
        while let Ok(data_publisher) = receiver.lock().await.recv().await {
            let mut res = false;
            {
                let data_publisher = data_publisher.lock().await;
                if data_publisher.label == label {
                    res = true;
                }
            }
            if res {
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
        if self.handler.signaling_state() != RTCSignalingState::Stable {
            return Err(Error::new_transport(
                format!("Signaling state is {}", self.handler.signaling_state()),
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

    pub async fn restart_ice(&self) -> Result<(), Error> {
        tracing::debug!("publisher restarting ice");
        let state = self.handler.connection_state();
        if state == RTCPeerConnectionState::New || state == RTCPeerConnectionState::Closed {
            return Err(Error::new_transport(
                format!("Connection state is not correct: {}", state),
                TransportErrorKind::ICERestartError,
            ));
        }
        let _ = self.peer_connection.restart_ice().await?;
        Ok(())
    }

    pub async fn get_local_description(&self) -> Option<RTCSessionDescription> {
        self.peer_connection.local_description().await
    }

    pub async fn get_remote_description(&self) -> Option<RTCSessionDescription> {
        self.peer_connection.remote_description().await
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

    async fn cleanup(
        published_channel: Arc<replay_channel::ReplayChannel<Arc<Mutex<Publisher>>>>,
        published_receiver: Arc<Mutex<mpsc::Receiver<Arc<Mutex<Publisher>>>>>,
    ) {
        published_channel.clear().await;

        {
            let mut rx = published_receiver.lock().await;
            rx.close();
            while rx.try_recv().is_ok() {}
        }
    }

    pub async fn close(&self) -> Result<(), Error> {
        self.peer_connection.close().await?;
        Self::cleanup(
            self.published_channel.clone(),
            self.published_receiver.clone(),
        )
        .await;

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

    fn signaling_state(&self) -> RTCSignalingState {
        self.handler.signaling_state()
    }

    fn ice_gathering_state(&self) -> RTCIceGatheringState {
        self.handler.ice_gathering_state()
    }

    fn connection_state(&self) -> RTCPeerConnectionState {
        self.handler.connection_state()
    }

    async fn get_stats(&self) -> RTCStatsReport {
        let report = self
            .peer_connection
            .get_stats(Instant::now(), StatsSelector::None)
            .await;
        report
    }
}

impl Drop for PublishTransport {
    fn drop(&mut self) {
        tracing::debug!("PublishTransport {} is dropped", self.id);
    }
}

#[derive(Clone)]
struct PublishHandler {
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    on_ice_candidate_fn: Arc<Mutex<OnIceCandidateFn>>,
    on_track_fn: Arc<Mutex<OnTrackFn>>,
    relay_sender: Arc<RelaySender>,
    private_ip: String,
    published_channel: Arc<replay_channel::ReplayChannel<Arc<Mutex<Publisher>>>>,
    published_receiver: Arc<Mutex<mpsc::Receiver<Arc<Mutex<Publisher>>>>>,
    data_published_sender: broadcast::Sender<Arc<Mutex<DataPublisher>>>,
    signaling_pending: Arc<AtomicBool>,
    signaling_state: Arc<RwLock<RTCSignalingState>>,
    connection_state: Arc<RwLock<RTCPeerConnectionState>>,
    ice_gathering_state: Arc<RwLock<RTCIceGatheringState>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for PublishHandler {
    // This callback is called after initializing PeerConnection with ICE servers.
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let locked = self.on_ice_candidate_fn.lock().await;
        let candidate = event.candidate;
        tracing::info!("on ice candidate: {}", candidate);
        // Call on_ice_candidate_fn as callback.
        (locked)(candidate);
    }

    async fn on_negotiation_needed(&self) {
        tracing::error!("on negotiation needed in publisher");
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let id = track.track_id().await;
        tracing::info!("Track published: track_id={}", id);

        let publisher = Publisher::new(
            track.clone(),
            self.router_event_sender.clone(),
            self.relay_sender.clone(),
            self.private_ip.clone(),
        )
        .await;

        self.published_channel.send(publisher.clone()).await;

        let locked = self.on_track_fn.lock().await;
        (locked)(track);
    }

    async fn on_data_channel(&self, data_channel: Arc<dyn DataChannel>) {
        let p = match DataPublisher::new(
            data_channel,
            self.router_event_sender.clone(),
            self.relay_sender.clone(),
        )
        .await
        {
            Ok(p) => p,
            Err(err) => {
                tracing::error!("Failed to create data publisher: {}", err);
                return;
            }
        };

        let data_publisher = Arc::new(Mutex::new(p));
        self.data_published_sender
            .send(data_publisher.clone())
            .expect("could not send data published to publisher");
        let _ = self
            .router_event_sender
            .send(RouterEvent::DataPublished(data_publisher));
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        tracing::debug!("ICE gathering state changed: {}", state);
        *self.ice_gathering_state.write().unwrap() = state;
    }

    async fn on_signaling_state_change(&self, state: RTCSignalingState) {
        tracing::debug!("Signaling state changed: {}", state);
        if state == RTCSignalingState::Stable {
            self.signaling_pending.store(false, Ordering::Relaxed);
        }
        *self.signaling_state.write().unwrap() = state;
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        *self.connection_state.write().unwrap() = state;
        if state == RTCPeerConnectionState::Closed || state == RTCPeerConnectionState::Failed {
            PublishTransport::cleanup(
                self.published_channel.clone(),
                self.published_receiver.clone(),
            )
            .await;
        }
    }
}

impl PublishHandler {
    fn signaling_state(&self) -> RTCSignalingState {
        *self.signaling_state.read().unwrap()
    }

    fn ice_gathering_state(&self) -> RTCIceGatheringState {
        *self.ice_gathering_state.read().unwrap()
    }

    fn connection_state(&self) -> RTCPeerConnectionState {
        *self.connection_state.read().unwrap()
    }
}

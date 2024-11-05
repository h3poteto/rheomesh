use std::sync::Arc;

use enclose::enc;
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;

use webrtc::{
    peer_connection::{
        offer_answer_options::RTCOfferOptions, sdp::session_description::RTCSessionDescription,
    },
    track::track_local::track_local_static_rtp::TrackLocalStaticRTP,
};

use crate::config::{MediaConfig, WebRTCTransportConfig};
use crate::data_publisher::DataPublisher;
use crate::data_subscriber::DataSubscriber;
use crate::subscriber::Subscriber;
use crate::transport::{OnIceCandidateFn, OnNegotiationNeededFn, Transport};
use crate::{
    error::{Error, SubscriberErrorKind},
    publisher::Publisher,
    router::RouterEvent,
};

#[derive(Clone)]
pub struct SubscribeTransport {
    pub id: String,
    peer_connection: Arc<RTCPeerConnection>,
    pending_candidates: Arc<Mutex<Vec<RTCIceCandidateInit>>>,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    offer_options: RTCOfferOptions,
    // For callback fn
    on_ice_candidate_fn: Arc<Mutex<OnIceCandidateFn>>,
    on_negotiation_needed_fn: Arc<Mutex<OnNegotiationNeededFn>>,
    // rtp event
    closed_sender: Arc<mpsc::UnboundedSender<bool>>,
    closed_receiver: Arc<Mutex<mpsc::UnboundedReceiver<bool>>>,
}

impl SubscribeTransport {
    pub async fn new(
        router_event_sender: mpsc::UnboundedSender<RouterEvent>,
        media_config: MediaConfig,
        transport_config: WebRTCTransportConfig,
    ) -> Arc<Self> {
        let id = Uuid::new_v4().to_string();

        let peer_connection = Self::generate_peer_connection(media_config, transport_config)
            .await
            .unwrap();

        let (closed_sender, closed_receiver) = mpsc::unbounded_channel();

        let mut transport = Self {
            id,
            peer_connection: Arc::new(peer_connection),
            router_event_sender,
            offer_options: RTCOfferOptions {
                ice_restart: false,
                voice_activity_detection: false,
            },
            pending_candidates: Arc::new(Mutex::new(Vec::new())),
            on_ice_candidate_fn: Arc::new(Mutex::new(Box::new(|_| {}))),
            on_negotiation_needed_fn: Arc::new(Mutex::new(Box::new(|_| {}))),
            closed_sender: Arc::new(closed_sender),
            closed_receiver: Arc::new(Mutex::new(closed_receiver)),
        };

        transport.ice_state_hooks().await;

        let subscriber = Arc::new(transport);

        tracing::debug!("SubscribeTransport {} is created", subscriber.id);

        subscriber
    }

    pub async fn subscribe(
        &self,
        publisher_id: String,
    ) -> Result<(Subscriber, RTCSessionDescription), Error> {
        // We have to add a track before creating offer.
        // https://datatracker.ietf.org/doc/html/rfc3264
        // https://github.com/webrtc-rs/webrtc/issues/115#issuecomment-1958137875
        let (tx, rx) = oneshot::channel();

        let _ = self
            .router_event_sender
            .send(RouterEvent::GetPublisher(publisher_id.clone(), tx));

        let reply = rx.await.unwrap();
        match reply {
            None => {
                return Err(Error::new_subscriber(
                    format!("Publisher for {} is not found", publisher_id),
                    SubscriberErrorKind::TrackNotFoundError,
                ))
            }
            Some(publisher) => {
                let subscriber = self.subscribe_track(publisher).await?;

                let offer = self.create_offer().await?;
                Ok((subscriber, offer))
            }
        }
    }

    pub async fn data_subscribe(
        &self,
        data_publisher_id: String,
    ) -> Result<(DataSubscriber, RTCSessionDescription), Error> {
        let (tx, rx) = oneshot::channel();

        let _ = self
            .router_event_sender
            .send(RouterEvent::GetDataPublisher(data_publisher_id.clone(), tx));

        let reply = rx.await.unwrap();
        match reply {
            None => Err(Error::new_subscriber(
                format!("DataPublisher for {} is not found", data_publisher_id),
                SubscriberErrorKind::DataChannelNotFoundError,
            )),
            Some(data_publisher) => {
                let data_subscriber = self.subscribe_data(data_publisher).await?;

                let offer = self.create_offer().await?;
                Ok((data_subscriber, offer))
            }
        }
    }

    async fn create_offer(&self) -> Result<RTCSessionDescription, Error> {
        tracing::debug!("subscriber creates offer");

        let offer = self
            .peer_connection
            .create_offer(Some(self.offer_options.clone()))
            .await?;

        let mut gathering_complete = self.peer_connection.gathering_complete_promise().await;
        self.peer_connection.set_local_description(offer).await?;
        let _ = gathering_complete.recv().await;

        match self.peer_connection.local_description().await {
            Some(offer) => Ok(offer),
            None => Err(Error::new_transport(
                "Failed to set local description".to_string(),
                crate::error::TransportErrorKind::LocalDescriptionError,
            )),
        }
    }

    pub async fn set_answer(&self, answer: RTCSessionDescription) -> Result<(), Error> {
        tracing::debug!("subscriber set answer");
        self.peer_connection.set_remote_description(answer).await?;

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

        Ok(())
    }

    async fn subscribe_track(&self, publisher: Arc<Publisher>) -> Result<Subscriber, Error> {
        let publisher_rtcp_sender = publisher.rtcp_sender.clone();
        let track_id = publisher.track.id();
        let local_track = TrackLocalStaticRTP::new(
            publisher.track.codec().capability,
            publisher.track.id(),
            publisher.track.stream_id(),
        );
        let mime_type = publisher.track.codec().capability.mime_type;

        let local_track = Arc::new(local_track);

        let rtcp_sender = self.peer_connection.add_track(local_track.clone()).await?;
        let media_ssrc = publisher.track.ssrc();
        let rtp_buffer = publisher.rtp_sender.clone();
        let peer = self.peer_connection.clone();
        let closed_receiver = self.closed_receiver.clone();

        let subscriber = Subscriber::new(
            rtcp_sender,
            publisher_rtcp_sender,
            mime_type,
            media_ssrc,
            local_track,
            rtp_buffer,
            track_id,
            closed_receiver,
            peer,
        );

        Ok(subscriber)
    }

    async fn subscribe_data(
        &self,
        data_publisher: Arc<DataPublisher>,
    ) -> Result<DataSubscriber, Error> {
        let data_sender = data_publisher.data_sender.clone();

        let data_channel = self
            .peer_connection
            .create_data_channel(data_publisher.id.as_str(), None)
            .await?;

        let closed_receiver = self.closed_receiver.clone();
        let data_subscriber = DataSubscriber::new(
            data_publisher.id.clone(),
            data_channel,
            data_sender,
            closed_receiver,
        );

        Ok(data_subscriber)
    }

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

        let downgraded_peer = Arc::downgrade(&peer);
        let on_negotiation_needed = Arc::clone(&self.on_negotiation_needed_fn);
        peer.on_negotiation_needed(Box::new(enc!( (downgraded_peer, on_negotiation_needed) move || {
                Box::pin(enc!( (downgraded_peer, on_negotiation_needed) async move {
                    tracing::info!("on negotiation needed");
                    let locked = on_negotiation_needed.lock().await;
                    if let Some(pc) = downgraded_peer.upgrade() {
                        if pc.connection_state() == RTCPeerConnectionState::Closed {
                                return;
                        }

                        let offer = pc.create_offer(None).await.expect("could not create subscriber offer:");
                        pc.set_local_description(offer).await.expect("could not set local description");
                        let offer = pc.local_description().await.unwrap();

                        tracing::info!("peer sending offer");
                        (locked)(offer);
                    }
                }))
            })));

        peer.on_ice_gathering_state_change(Box::new(move |state| {
            Box::pin(async move {
                tracing::debug!("ICE gathering state changed: {}", state);
            })
        }));
    }

    // Hooks
    pub async fn on_ice_candidate(&self, f: OnIceCandidateFn) {
        let mut callback = self.on_ice_candidate_fn.lock().await;
        *callback = f;
    }

    pub async fn on_negotiation_needed(&self, f: OnNegotiationNeededFn) {
        let mut callback = self.on_negotiation_needed_fn.lock().await;
        *callback = f;
    }

    pub async fn close(&self) -> Result<(), Error> {
        self.closed_sender.send(true).unwrap();

        self.peer_connection.close().await?;
        Ok(())
    }
}

impl Transport for SubscribeTransport {
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

impl Drop for SubscribeTransport {
    fn drop(&mut self) {
        tracing::debug!("SubscribeTransport {} is dropped", self.id);
    }
}

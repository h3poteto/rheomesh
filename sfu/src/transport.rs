use crate::{
    config::WebRTCTransportConfig,
    data_track::DataTrack,
    error::{Error, TransportErrorKind},
    media_engine,
    media_track::MediaTrack,
    router::RouterEvent,
};
use enclose::enc;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};
use uuid::Uuid;
use webrtc::{
    api::{media_engine::MediaEngine, APIBuilder},
    data_channel::{data_channel_init::RTCDataChannelInit, RTCDataChannel},
    ice_transport::{
        ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
        ice_gatherer_state::RTCIceGathererState,
    },
    peer_connection::{
        offer_answer_options::{RTCAnswerOptions, RTCOfferOptions},
        peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription,
        RTCPeerConnection,
    },
    rtcp,
    rtp_transceiver::{rtp_receiver::RTCRtpReceiver, rtp_sender::RTCRtpSender, RTCRtpTransceiver},
    track::{track_local::TrackLocal, track_remote::TrackRemote},
};

pub type RtcpSender = mpsc::UnboundedSender<Box<dyn rtcp::packet::Packet + Send + Sync>>;
pub type RtcpReceiver = mpsc::UnboundedReceiver<Box<dyn rtcp::packet::Packet + Send + Sync>>;

pub type OnIceCandidateFn = Box<dyn Fn(RTCIceCandidate) + Send + Sync>;
pub type OnNegotiationNeededFn = Box<dyn Fn(RTCSessionDescription) + Send + Sync>;
pub type OnTrackFn =
    Box<dyn Fn(Arc<TrackRemote>, Arc<RTCRtpReceiver>, Arc<RTCRtpTransceiver>) + Send + Sync>;

#[derive(Clone)]
pub struct Transport {
    pub id: String,
    pub router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    config: WebRTCTransportConfig,
    peer_connection: Option<Arc<RTCPeerConnection>>,
    rtcp_sender_channel: Arc<RtcpSender>,
    rtcp_receiver_channel: Arc<Mutex<RtcpReceiver>>,
    stop_sender_channel: Arc<Mutex<mpsc::UnboundedSender<()>>>,
    stop_receiver_channel: Arc<Mutex<mpsc::UnboundedReceiver<()>>>,
    on_ice_candidate_fn: Arc<Mutex<OnIceCandidateFn>>,
    on_negotiation_needed_fn: Arc<Mutex<OnNegotiationNeededFn>>,
    on_track_fn: Arc<Mutex<OnTrackFn>>,
    ice_gathering_complete_sender: mpsc::UnboundedSender<()>,
    pub ice_gathering_complete_receiver: Arc<Mutex<mpsc::UnboundedReceiver<()>>>,
    pending_candidates: Arc<Mutex<Vec<RTCIceCandidateInit>>>,
    published_sender: broadcast::Sender<String>,
}

impl Transport {
    pub async fn new(
        router_event_sender: mpsc::UnboundedSender<RouterEvent>,
        config: WebRTCTransportConfig,
    ) -> Transport {
        let id = Uuid::new_v4().to_string();
        let (s, r) = mpsc::unbounded_channel();
        let (stop_sender, stop_receiver) = mpsc::unbounded_channel();
        let (ice_sender, ice_receiver) = mpsc::unbounded_channel();
        let (published_sender, _) = broadcast::channel(1024);

        let mut transport = Transport {
            id,
            router_event_sender,
            config,
            peer_connection: None,
            rtcp_sender_channel: Arc::new(s),
            rtcp_receiver_channel: Arc::new(Mutex::new(r)),
            stop_sender_channel: Arc::new(Mutex::new(stop_sender)),
            stop_receiver_channel: Arc::new(Mutex::new(stop_receiver)),
            on_ice_candidate_fn: Arc::new(Mutex::new(Box::new(|_| {}))),
            on_negotiation_needed_fn: Arc::new(Mutex::new(Box::new(|_| {}))),
            on_track_fn: Arc::new(Mutex::new(Box::new(|_, _, _| {}))),
            ice_gathering_complete_sender: ice_sender,
            ice_gathering_complete_receiver: Arc::new(Mutex::new(ice_receiver)),
            pending_candidates: Arc::new(Mutex::new(Vec::new())),
            published_sender,
        };

        let _ = transport.build_transport().await;
        transport.ice_state_hooks().await;

        transport
    }

    pub(crate) fn get_published_receiver(&self) -> broadcast::Receiver<String> {
        let subscriber = self.published_sender.subscribe();
        subscriber
    }

    pub(crate) async fn set_remote_description(
        &self,
        sdp: RTCSessionDescription,
    ) -> Result<(), Error> {
        let peer = self.peer_connection.clone().ok_or(Error::new_transport(
            "PeerConnection does not exist".to_string(),
            TransportErrorKind::PeerConnectionError,
        ))?;

        tracing::debug!("Set remote description");
        let res = peer.set_remote_description(sdp).await?;
        let pendings = self.pending_candidates.lock().await;
        for candidate in pendings.iter() {
            tracing::debug!("Adding pending ICE candidate: {:#?}", candidate);
            if let Err(err) = peer.add_ice_candidate(candidate.clone()).await {
                tracing::error!("failed to add_ice_candidate: {}", err);
            }
        }

        Ok(res)
    }

    pub(crate) async fn set_local_description(
        &self,
        sdp: RTCSessionDescription,
    ) -> Result<(), Error> {
        let peer = self.peer_connection.clone().ok_or(Error::new_transport(
            "PeerConnection does not exist".to_string(),
            TransportErrorKind::PeerConnectionError,
        ))?;

        let res = peer.set_local_description(sdp).await?;
        Ok(res)
    }

    pub(crate) async fn create_offer(
        &self,
        options: Option<RTCOfferOptions>,
    ) -> Result<RTCSessionDescription, Error> {
        let peer = self.peer_connection.clone().ok_or(Error::new_transport(
            "PeerConnection does not exist".to_string(),
            TransportErrorKind::PeerConnectionError,
        ))?;

        let offer = peer.create_offer(options).await?;
        Ok(offer)
    }

    pub(crate) async fn create_answer(
        &self,
        options: Option<RTCAnswerOptions>,
    ) -> Result<RTCSessionDescription, Error> {
        let peer = self.peer_connection.clone().ok_or(Error::new_transport(
            "PeerConnection does not exist".to_string(),
            TransportErrorKind::PeerConnectionError,
        ))?;

        let answer = peer.create_answer(options).await?;
        Ok(answer)
    }

    pub(crate) async fn local_description(&self) -> Result<Option<RTCSessionDescription>, Error> {
        let peer = self.peer_connection.clone().ok_or(Error::new_transport(
            "PeerConnection does not exist".to_string(),
            TransportErrorKind::PeerConnectionError,
        ))?;

        let answer = peer.local_description().await;
        Ok(answer)
    }

    // TODO: we don't use this
    pub(crate) async fn gathering_complete_promise(&self) -> Result<mpsc::Receiver<()>, Error> {
        let peer = self.peer_connection.clone().ok_or(Error::new_transport(
            "PeerConnection does not exist".to_string(),
            TransportErrorKind::PeerConnectionError,
        ))?;
        tracing::debug!("Waiting ICE gathering complete");

        let promise = peer.gathering_complete_promise().await;
        Ok(promise)
    }

    pub async fn add_ice_candidate(&self, candidate: RTCIceCandidateInit) -> Result<(), Error> {
        let peer = self.peer_connection.clone().ok_or(Error::new_transport(
            "PeerConnection does not exist".to_string(),
            TransportErrorKind::PeerConnectionError,
        ))?;

        tracing::debug!("Adding ICE candidate for {:#?}", candidate);

        if let Some(_rd) = peer.remote_description().await {
            let _ = peer.add_ice_candidate(candidate.clone()).await?;
        } else {
            self.pending_candidates.lock().await.push(candidate.clone());
        }

        Ok(())
    }

    pub(crate) async fn add_track(
        &self,
        track: Arc<dyn TrackLocal + Send + Sync>,
    ) -> Result<Arc<RTCRtpSender>, Error> {
        let peer = self.peer_connection.clone().ok_or(Error::new_transport(
            "PeerConnection does not exist".to_string(),
            TransportErrorKind::PeerConnectionError,
        ))?;

        let res = peer.add_track(track).await?;
        Ok(res)
    }

    pub(crate) async fn create_data_channel(
        &self,
        label: &str,
        options: Option<RTCDataChannelInit>,
    ) -> Result<Arc<RTCDataChannel>, Error> {
        let peer = self.peer_connection.clone().ok_or(Error::new_transport(
            "PeerConnection does not exist".to_string(),
            TransportErrorKind::PeerConnectionError,
        ))?;

        let res = peer.create_data_channel(label, options).await?;
        Ok(res)
    }

    pub async fn close(self) -> Result<(), Error> {
        if let Err(err) = self.stop_sender_channel.lock().await.send(()) {
            tracing::error!("failed to stop rtcp writer loop: {}", err);
        }
        match self.peer_connection {
            Some(peer) => {
                peer.close().await?;
                Ok(())
            }
            None => Ok(()),
        }
    }

    async fn build_transport(&mut self) -> Result<(), webrtc::error::Error> {
        let mut me = MediaEngine::default();
        media_engine::register_default_codecs(&mut me)?;
        media_engine::register_extensions(&mut me)?;
        let api = APIBuilder::new()
            .with_media_engine(me)
            .with_setting_engine(self.config.setting_engine.clone())
            .build();

        let peer_connection = api
            .new_peer_connection(self.config.configuration.clone())
            .await?;

        let pc = Some(Arc::new(peer_connection));
        self.peer_connection = pc.clone();

        if let Some(pc) = pc {
            let rtcp_receiver = self.rtcp_receiver_channel.clone();
            let stop_receiver = self.stop_receiver_channel.clone();
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

        Ok(())
    }

    async fn ice_state_hooks(&mut self) {
        let peer = self.peer_connection.clone().unwrap();
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
                    published_sender.send(id.clone()).expect("could not send published track id to publisher");
                    let (media_track, closed) = MediaTrack::new(track.clone(), receiver.clone(), transceiver.clone(), rtcp_sender);
                    let _ = router_sender.send(RouterEvent::TrackPublished(media_track));

                    (locked)(track, receiver, transceiver);

                    // Keep this thread until closed, and send TrackRemove event
                    let _ = closed.await;
                    let _ = router_sender.send(RouterEvent::TrackRemoved(id));
                }))
            }
        )));

        let sender = self.ice_gathering_complete_sender.clone();
        peer.on_ice_gathering_state_change(enc!((sender) Box::new(move |state| {
            Box::pin(enc!((sender) async move {
                tracing::debug!("ICE gathering state changed: {}", state);
                if state == RTCIceGathererState::Complete {
                    let _ = sender.send(());
                }
            }))
        })));

        let router_sender = self.router_event_sender.clone();
        peer.on_data_channel(Box::new(
            enc!((router_sender) move |dc: Arc<RTCDataChannel>| {
                Box::pin(enc!((router_sender) async move {
                    let label = dc.label();
                    let id = dc.id();
                    tracing::info!("DataChannel opened: label={}, id={}", label, id);
                    let (data_track, closed) = DataTrack::new(dc.clone());
                    let _ = router_sender.send(RouterEvent::DataChannelOpened(data_track));

                    let _ = closed.await;
                    let _ = router_sender.send(RouterEvent::DataChannelClosed(id));
                }))
            }),
        ));
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

    pub async fn on_track(&mut self, f: OnTrackFn) {
        let mut callback = self.on_track_fn.lock().await;
        *callback = f;
    }
}

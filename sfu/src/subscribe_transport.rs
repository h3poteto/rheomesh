use std::time::Duration;
use std::{
    sync::{
        Arc, OnceLock, RwLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use derivative::Derivative;
use rand::random;
use rtc::statistics::StatsSelector;
use rtc::{
    media_stream::MediaStreamTrack,
    peer_connection::configuration::{RTCOfferOptions, media_engine::MIME_TYPE_OPUS},
    rtp_transceiver::rtp_sender::{
        RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
    },
};
use tokio::sync::oneshot;
use tokio::{
    sync::{Mutex, mpsc, watch},
    time::sleep,
};
use uuid::Uuid;
use webrtc::{
    media_stream::track_local::{
        static_rtp::TrackLocalStaticRTP, static_sample::TrackLocalStaticSample,
    },
    peer_connection::{
        PeerConnection, PeerConnectionEventHandler, RTCIceCandidateInit, RTCIceGatheringState,
        RTCPeerConnectionIceEvent, RTCPeerConnectionState, RTCSessionDescription,
        RTCSignalingState, RTCStatsReport,
    },
};
use webrtc_sdp::attribute_type::{SdpAttribute, SdpAttributeType};
use webrtc_sdp::parse_sdp;

use crate::error::SubscriberErrorKind;
use crate::{
    config::{MediaConfig, RID, WebRTCTransportConfig, find_extmap_order},
    data_channel::Channel,
    data_subscriber::DataSubscriber,
    error::{Error, TransportErrorKind},
    prober::Prober,
    router::{Router, RouterEvent},
    subscriber::Subscriber,
    track::Track,
    transport::{self, OnIceCandidateFn, OnNegotiationNeededFn, Transport},
};

/// This handle [`webrtc::peer_connection::RTCPeerConnection`] methods for subscriber.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct SubscribeTransport {
    pub id: String,
    #[derivative(Debug = "ignore")]
    peer_connection: Arc<dyn PeerConnection>,
    pending_candidates: Arc<Mutex<Vec<RTCIceCandidateInit>>>,
    pub(crate) router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    offer_options: RTCOfferOptions,
    // For callback fn
    #[derivative(Debug = "ignore")]
    on_ice_candidate_fn: Arc<Mutex<OnIceCandidateFn>>,
    #[derivative(Debug = "ignore")]
    on_negotiation_needed_fn: Arc<Mutex<OnNegotiationNeededFn>>,
    // rtp event
    closed_sender: watch::Sender<bool>,
    closed_receiver: watch::Receiver<bool>,
    signaling_pending: Arc<AtomicBool>,
    #[derivative(Debug = "ignore")]
    handler: Arc<SubscribeHandler>,
}

impl SubscribeTransport {
    pub(crate) async fn new(
        router_event_sender: mpsc::UnboundedSender<RouterEvent>,
        media_config: MediaConfig,
        transport_config: WebRTCTransportConfig,
    ) -> Self {
        let id = Uuid::new_v4().to_string();

        let on_ice_candidate_fn: Arc<Mutex<OnIceCandidateFn>> =
            Arc::new(Mutex::new(Box::new(|_| {})));
        let on_negotiation_needed_fn: Arc<Mutex<OnNegotiationNeededFn>> =
            Arc::new(Mutex::new(Box::new(|_| {})));
        let offer_options = RTCOfferOptions { ice_restart: false };
        let signaling_pending = Arc::new(AtomicBool::new(false));
        let (closed_sender, closed_receiver) = watch::channel(false);

        let handler = Arc::new(SubscribeHandler {
            peer_connection: Arc::new(OnceLock::new()),
            on_ice_candidate_fn: on_ice_candidate_fn.clone(),
            on_negotiation_needed_fn: on_negotiation_needed_fn.clone(),
            offer_options: offer_options.clone(),
            signaling_pending: signaling_pending.clone(),
            closed_sender: closed_sender.clone(),
            signaling_state: Arc::new(RwLock::new(RTCSignalingState::default())),
            ice_gathering_state: Arc::new(RwLock::new(RTCIceGatheringState::default())),
            connection_state: Arc::new(RwLock::new(RTCPeerConnectionState::default())),
            gathering_complete: Arc::new(std::sync::Mutex::new(None)),
        });

        let peer_connection =
            transport::generate_peer_connection(handler.clone(), media_config, transport_config)
                .await
                .unwrap();

        let _ = handler
            .peer_connection
            .set(Arc::downgrade(&peer_connection));

        let transport = Self {
            id,
            peer_connection,
            router_event_sender,
            offer_options,
            pending_candidates: Arc::new(Mutex::new(Vec::new())),
            on_ice_candidate_fn,
            on_negotiation_needed_fn,
            closed_sender,
            closed_receiver,
            signaling_pending,
            handler,
        };

        tracing::debug!("SubscribeTransport {} is created", transport.id);

        transport
    }

    /// This starts subscribing the published media and returns an offer sdp. Please provide a [`crate::publisher::Publisher`] ID.
    pub async fn subscribe(
        &self,
        publisher_id: String,
    ) -> Result<(Arc<Mutex<Subscriber>>, RTCSessionDescription), Error> {
        // We have to add a track before creating offer.
        // https://datatracker.ietf.org/doc/html/rfc3264
        // https://github.com/webrtc-rs/webrtc/issues/115#issuecomment-1958137875
        let local_track = self
            .find_local_track(publisher_id.clone(), RID::HIGH)
            .await?;
        while self.signaling_pending.load(Ordering::Relaxed) {
            sleep(Duration::from_millis(10)).await;
        }
        self.signaling_pending.store(true, Ordering::Relaxed);
        let subscriber = self.subscribe_track(publisher_id, local_track).await?;
        let offer = self.create_offer().await?;
        Ok((subscriber, offer))
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

    /// This starts subscribing the data channel and returns an offer sdp. Please provide a [`crate::data_publisher::DataPublisher`] ID.
    pub async fn data_subscribe(
        &self,
        data_publisher_id: String,
    ) -> Result<(DataSubscriber, RTCSessionDescription), Error> {
        match self.find_data_publisher(data_publisher_id.clone()).await {
            Ok(data_publisher) => {
                let data_subscriber = self.subscribe_data(data_publisher).await?;

                let offer = self.create_offer().await?;
                Ok((data_subscriber, offer))
            }
            Err(err) => Err(err),
        }
    }

    async fn find_data_publisher(
        &self,
        data_publisher_id: String,
    ) -> Result<Arc<Mutex<dyn Channel>>, Error> {
        match Router::find_local_data_publisher(
            self.router_event_sender.clone(),
            data_publisher_id.clone(),
        )
        .await
        {
            Ok(data_publisher) => Ok(data_publisher),
            Err(_) => {
                match Router::find_relayed_data_publisher(
                    self.router_event_sender.clone(),
                    data_publisher_id,
                )
                .await
                {
                    Ok(relayed_data_publisher) => Ok(relayed_data_publisher),
                    Err(err) => Err(err),
                }
            }
        }
    }

    async fn create_offer(&self) -> Result<RTCSessionDescription, Error> {
        tracing::debug!("subscriber creates offer");

        let offer = self
            .peer_connection
            .create_offer(Some(self.offer_options.clone()))
            .await?;

        let gathering_complete = self.handler.trap_gathering_complete();
        self.peer_connection.set_local_description(offer).await?;
        let _ = gathering_complete.await;

        match self.peer_connection.local_description().await {
            Some(offer) => {
                let offer = adjust_extmap(offer)?;
                Ok(offer)
            }
            None => Err(Error::new_transport(
                "Failed to set local description".to_string(),
                TransportErrorKind::LocalDescriptionError,
            )),
        }
    }

    /// This sets the answer to the [`webrtc::peer_connection::RTCPeerConnection`].
    pub async fn set_answer(&self, answer: RTCSessionDescription) -> Result<(), Error> {
        tracing::debug!("subscriber set answer");
        self.peer_connection.set_remote_description(answer).await?;

        self.signaling_pending.store(false, Ordering::Relaxed);
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

    /// Set an empty offer and get a corresponding SDP answer.
    pub async fn get_answer(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<RTCSessionDescription, Error> {
        tracing::debug!("subscriber set offer");
        self.peer_connection.set_remote_description(offer).await?;

        let answer = self.peer_connection.create_answer(None).await?;

        let gathering_complete = self.handler.trap_gathering_complete();
        self.peer_connection.set_local_description(answer).await?;
        let _ = gathering_complete.await;

        match self.peer_connection.local_description().await {
            Some(answer) => {
                let answer = adjust_extmap(answer)?;
                Ok(answer)
            }
            None => Err(Error::new_transport(
                "Failed to set local description".to_string(),
                TransportErrorKind::LocalDescriptionError,
            )),
        }
    }

    pub async fn subscribe_track(
        &self,
        publisher_id: String,
        local_track: Arc<dyn Track>,
    ) -> Result<Arc<Mutex<Subscriber>>, Error> {
        let publisher_rtcp_sender = local_track.rtcp_sender().clone();
        let codec = local_track.capability();

        let ssrc = random::<u32>();
        let track_local_rtp = Arc::new(TrackLocalStaticRTP::new(media_stream_track(
            &local_track,
            ssrc,
        )));

        let track_local_rtp_sender = self
            .peer_connection
            .add_track(track_local_rtp.clone())
            .await?;
        let publisher_ssrc = local_track.ssrc();
        let rtp_packet_sender = local_track.rtp_packet_sender();
        let closed_receiver = self.closed_receiver.clone();

        let (subscriber, event_sender) = Subscriber::new(
            publisher_id.clone(),
            track_local_rtp,
            rtp_packet_sender,
            publisher_rtcp_sender,
            track_local_rtp_sender,
            codec,
            publisher_ssrc,
            self.router_event_sender.clone(),
            closed_receiver,
            ssrc,
        );

        {
            let router_event_sender = self.router_event_sender.clone();
            tokio::spawn(async move {
                if let Ok(publisher) =
                    Router::find_publisher(router_event_sender, publisher_id).await
                {
                    let mut guard = publisher.lock().await;
                    guard.set_subscriber_event_sender(event_sender);
                }
            });
        }

        if let None = self.peer_connection.current_local_description().await {
            let _ = self.add_probe().await?;
        };

        Ok(subscriber)
    }

    async fn subscribe_data(
        &self,
        data_publisher: Arc<Mutex<dyn Channel>>,
    ) -> Result<DataSubscriber, Error> {
        let data_publisher = data_publisher.lock().await;
        let data_sender = data_publisher.data_sender().clone();

        let data_channel = self
            .peer_connection
            .create_data_channel(data_publisher.id().as_str(), None)
            .await?;

        let closed_receiver = self.closed_receiver.clone();
        let data_subscriber = DataSubscriber::new(
            data_publisher.id().clone(),
            data_channel,
            data_sender,
            closed_receiver,
        );

        Ok(data_subscriber)
    }

    async fn add_probe(&self) -> Result<(), Error> {
        let codec = RTCRtpCodec {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48000,
            channels: 2,
            ..Default::default()
        };
        let ssrc = random::<u32>();
        let dummy_track = Arc::new(TrackLocalStaticSample::new(MediaStreamTrack::new(
            "webrtc-rs".to_owned(),
            "probator".to_owned(),
            "probator".to_owned(),
            RtpCodecKind::Audio,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(ssrc),
                    ..Default::default()
                },
                codec,
                active: true,
                ..Default::default()
            }],
        ))?);
        let rtp_sender = self.peer_connection.add_track(dummy_track.clone()).await?;
        let _prober = Prober::new(dummy_track, rtp_sender, ssrc);

        Ok(())
    }

    /// This restarts ICE negotiation and returns a new offer sdp.
    pub async fn restart_ice(&self) -> Result<RTCSessionDescription, Error> {
        tracing::debug!("subscriber restarting ice");
        let state = self.handler.connection_state();
        if state == RTCPeerConnectionState::New || state == RTCPeerConnectionState::Closed {
            return Err(Error::new_transport(
                format!("Connection state is not correct: {}", state),
                TransportErrorKind::ICERestartError,
            ));
        }
        let _ = self.peer_connection.restart_ice().await?;

        let mut options = self.offer_options.clone();
        options.ice_restart = true;
        let offer = self.peer_connection.create_offer(Some(options)).await?;

        let gathering_complete = self.handler.trap_gathering_complete();
        self.peer_connection.set_local_description(offer).await?;
        let _ = gathering_complete.await;

        match self.peer_connection.local_description().await {
            Some(offer) => {
                let offer = adjust_extmap(offer)?;
                Ok(offer)
            }
            None => Err(Error::new_transport(
                "Failed to set local description".to_string(),
                TransportErrorKind::LocalDescriptionError,
            )),
        }
    }

    // Hooks
    /// Set callback function when the [`webrtc::peer_connection::RTCPeerConnection`] receives `on_ice_candidate` events.
    pub async fn on_ice_candidate(&self, f: OnIceCandidateFn) {
        let mut callback = self.on_ice_candidate_fn.lock().await;
        *callback = f;
    }

    /// Set callback function when the [`webrtc::peer_connection::RTCPeerConnection`] receives `on_negotiation_needed` events.
    pub async fn on_negotiation_needed(&self, f: OnNegotiationNeededFn) {
        let mut callback = self.on_negotiation_needed_fn.lock().await;
        *callback = f;
    }

    fn cleanup(closed_sender: watch::Sender<bool>) {
        let _ = closed_sender.send(true);
    }

    pub async fn close(&self) -> Result<(), Error> {
        Self::cleanup(self.closed_sender.clone());

        self.peer_connection.close().await?;
        Ok(())
    }
}

fn adjust_extmap(mut sdp: RTCSessionDescription) -> Result<RTCSessionDescription, Error> {
    let mut session = parse_sdp(&sdp.sdp, false)?;

    for media in session.media.iter_mut() {
        let mut found_attr = vec![];
        for attr in media.get_attributes() {
            match attr {
                SdpAttribute::Extmap(extmap) => {
                    found_attr.push(extmap.clone());
                }
                _ => continue,
            }
        }
        media.remove_attribute(SdpAttributeType::Extmap);
        for attr in found_attr {
            if let Some(order) = find_extmap_order(&attr.url) {
                let mut new_attr = attr.clone();
                new_attr.id = order;
                let _ = media.add_attribute(SdpAttribute::Extmap(new_attr))?;
            };
        }
    }
    tracing::trace!("updated session: {:#?}", session);
    sdp.sdp = session.to_string();
    Ok(sdp)
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

impl Drop for SubscribeTransport {
    fn drop(&mut self) {
        tracing::debug!("SubscribeTransport {} is dropped", self.id);
    }
}

#[derive(Clone)]
struct SubscribeHandler {
    peer_connection: Arc<OnceLock<Weak<dyn PeerConnection>>>,
    on_ice_candidate_fn: Arc<Mutex<OnIceCandidateFn>>,
    on_negotiation_needed_fn: Arc<Mutex<OnNegotiationNeededFn>>,
    offer_options: RTCOfferOptions,
    signaling_pending: Arc<AtomicBool>,
    closed_sender: watch::Sender<bool>,
    signaling_state: Arc<RwLock<RTCSignalingState>>,
    connection_state: Arc<RwLock<RTCPeerConnectionState>>,
    ice_gathering_state: Arc<RwLock<RTCIceGatheringState>>,
    gathering_complete: Arc<std::sync::Mutex<Option<oneshot::Sender<()>>>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for SubscribeHandler {
    // This callback is called after initializing PeerConnection with ICE servers.
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let locked = self.on_ice_candidate_fn.lock().await;
        let candidate = event.candidate;
        tracing::info!("on ice candidate: {}", candidate);
        // Call on_ice_candidate_fn as callback.
        (locked)(candidate);
    }

    async fn on_negotiation_needed(&self) {
        tracing::info!("on negotiation needed");
        let handler = self.clone();
        tokio::spawn(async move {
            handler.negotiate().await;
        });
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        tracing::debug!("ICE gathering state changed: {}", state);
        *self.ice_gathering_state.write().unwrap() = state;

        if state == RTCIceGatheringState::Complete {
            let sender = self.gathering_complete.lock().unwrap().take();
            if let Some(sender) = sender {
                let _ = sender.send(());
            }
        }
    }

    async fn on_signaling_state_change(&self, state: RTCSignalingState) {
        *self.signaling_state.write().unwrap() = state;
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        *self.connection_state.write().unwrap() = state;
        if state == RTCPeerConnectionState::Closed || state == RTCPeerConnectionState::Failed {
            let _ = self.closed_sender.send(true);
        }
    }
}

impl SubscribeHandler {
    fn signaling_state(&self) -> RTCSignalingState {
        *self.signaling_state.read().unwrap()
    }

    fn ice_gathering_state(&self) -> RTCIceGatheringState {
        *self.ice_gathering_state.read().unwrap()
    }

    fn connection_state(&self) -> RTCPeerConnectionState {
        *self.connection_state.read().unwrap()
    }

    fn trap_gathering_complete(&self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        *self.ice_gathering_state.write().unwrap() = RTCIceGatheringState::Gathering;
        *self.gathering_complete.lock().unwrap() = Some(tx);
        rx
    }

    async fn negotiate(&self) {
        while self.signaling_pending.load(Ordering::Relaxed) {
            sleep(Duration::from_millis(10)).await;
        }

        let Some(pc) = self.peer_connection.get().and_then(|weak| weak.upgrade()) else {
            return;
        };

        let locked = self.on_negotiation_needed_fn.lock().await;
        if self.connection_state() == RTCPeerConnectionState::Closed {
            tracing::info!("Skip negotiation because connection state is closed");
            return;
        }
        if self.signaling_state() != RTCSignalingState::Stable {
            tracing::info!(
                "Skip negotiation because signaling state is {}",
                self.signaling_state()
            );
            return;
        }
        self.signaling_pending.store(true, Ordering::Relaxed);
        match self.create_and_send_offer(&pc).await {
            Ok(offer) => {
                (locked)(offer);
            }
            Err(err) => {
                tracing::error!("Negotiation failed: {}", err);
                self.signaling_pending.store(false, Ordering::Relaxed);
            }
        }
    }

    async fn create_and_send_offer(
        &self,
        pc: &Arc<dyn PeerConnection>,
    ) -> Result<RTCSessionDescription, Error> {
        let offer = pc.create_offer(Some(self.offer_options.clone())).await?;
        let offer = adjust_extmap(offer)?;

        let gathering_complete = self.trap_gathering_complete();
        pc.set_local_description(offer).await?;
        let _ = gathering_complete.await;

        let offer = pc.local_description().await.ok_or(Error::new_subscriber(
            "local_description is empty".to_string(),
            SubscriberErrorKind::NoDescriptionError,
        ))?;

        tracing::info!("peer sending offer");
        Ok(offer)
    }
}

fn media_stream_track(track: &Arc<dyn Track>, ssrc: u32) -> MediaStreamTrack {
    let mime_type = track.mime_type();
    let kind = RtpCodecKind::from(
        mime_type
            .split("/")
            .next()
            .unwrap_or_default()
            .to_lowercase()
            .as_str(),
    );

    MediaStreamTrack::new(
        track.stream_id(),
        track.id(),
        track.id(),
        kind,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(ssrc),
                ..Default::default()
            },
            codec: track.capability(),
            active: true,
            ..Default::default()
        }],
    )
}

#[cfg(test)]
mod test {
    use std::fs;

    use webrtc_sdp::attribute_type::SdpAttributeExtmap;

    use super::*;

    fn check_extmap_index(original_sdp_path: &str, correct_sdp_path: &str) {
        let original = fs::read_to_string(original_sdp_path)
            .expect(format!("failed to open {}", original_sdp_path).as_str());
        let correct = fs::read_to_string(correct_sdp_path)
            .expect(format!("failed to open {}", correct_sdp_path).as_str());
        let mut original_sdp = RTCSessionDescription::default();
        original_sdp.sdp = original;
        let res = adjust_extmap(original_sdp).expect("failed to adjust extmap");

        let correct_session = parse_sdp(&correct, false).expect("failed to parse correct sdp");
        let response_session = parse_sdp(&res.sdp, false).expect("failed to parse response sdp");
        for media in response_session.media {
            let SdpAttribute::Mid(mid) = media
                .get_attribute(SdpAttributeType::Mid)
                .expect("failed to find mid")
            else {
                todo!()
            };

            let correct_media = correct_session
                .media
                .clone()
                .into_iter()
                .find(|m| {
                    let SdpAttribute::Mid(correct_mid) = m
                        .get_attribute(SdpAttributeType::Mid)
                        .expect("failed to find mid")
                    else {
                        todo!()
                    };
                    correct_mid == mid
                })
                .expect("failed to find correct media");

            let correct_extmaps: Vec<SdpAttributeExtmap> = correct_media
                .get_attributes()
                .iter()
                .filter_map(|a| {
                    if let SdpAttribute::Extmap(extmap) = a {
                        Some(extmap.clone())
                    } else {
                        None
                    }
                })
                .collect();

            let extmaps: Vec<SdpAttributeExtmap> = media
                .get_attributes()
                .iter()
                .filter_map(|a| {
                    if let SdpAttribute::Extmap(extmap) = a {
                        Some(extmap.clone())
                    } else {
                        None
                    }
                })
                .collect();

            for extmap in extmaps.iter() {
                let correct_extmap = correct_extmaps
                    .iter()
                    .find(|e| e.url == extmap.url)
                    .expect("failed to find correct extmap");

                assert_eq!(extmap.id, correct_extmap.id);
                assert_eq!(extmap.url, correct_extmap.url);
            }
        }
    }

    #[test]
    fn test_adjust_extmap_video() {
        check_extmap_index(
            "./test_data/sdp_video_original",
            "./test_data/sdp_video_correct",
        );
    }

    #[test]
    fn test_adjust_extmap_audio() {
        check_extmap_index(
            "./test_data/sdp_audio_original",
            "./test_data/sdp_audio_correct",
        );
    }

    #[test]
    fn test_adjust_extmap_audio_video() {
        check_extmap_index(
            "./test_data/sdp_audio_video_original",
            "./test_data/sdp_audio_video_correct",
        );
    }
}

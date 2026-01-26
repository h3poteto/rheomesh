use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU16, AtomicU32, Ordering},
};

use enclose::enc;
use tokio::sync::{Mutex, broadcast, mpsc};
use uuid::Uuid;
use webrtc::{
    rtcp::{
        self,
        header::{FORMAT_PLI, PacketType},
        payload_feedbacks::picture_loss_indication::PictureLossIndication,
    },
    rtp,
    rtp_transceiver::rtp_sender::RTCRtpSender,
    track::track_local::{TrackLocalWriter, track_local_static_rtp::TrackLocalStaticRTP},
};

use crate::{
    config::RID,
    error::Error,
    publisher::PublisherType,
    router::{Router, RouterEvent},
    rtp::layer::Layer,
    track::Track,
    transport,
};

#[derive(Debug)]
pub struct Subscriber {
    pub id: String,
    publisher_id: String,
    closed_sender: broadcast::Sender<bool>,
    replaced_sender: broadcast::Sender<bool>,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    subscriber_event_sender: mpsc::UnboundedSender<SubscriberEvent>,
    track_local: Arc<TrackLocalStaticRTP>,
    publisher_rtcp_sender: Arc<transport::RtcpSender>,
    rtcp_sender: Arc<RTCRtpSender>,
    sequence: Arc<AtomicU16>,
    timestamp: Arc<AtomicU32>,
    rtp_lock: Arc<Mutex<bool>>,
    rtcp_lock: Arc<Mutex<bool>>,
    media_ssrc: u32,
    spatial_layer: Arc<AtomicU8>,
    temporal_layer: Arc<AtomicU8>,
}

impl Subscriber {
    pub(crate) fn new(
        publisher_id: String,
        track_local: Arc<TrackLocalStaticRTP>,
        rtp_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
        rtcp_sender: Arc<RTCRtpSender>,
        publisher_rtcp_sender: Arc<transport::RtcpSender>,
        _mime_type: String,
        media_ssrc: u32,
        router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    ) -> (Arc<Mutex<Self>>, mpsc::UnboundedSender<SubscriberEvent>) {
        let id = Uuid::new_v4().to_string();
        let (tx, _rx) = broadcast::channel::<bool>(1);
        let (replaced_sender, _rx) = broadcast::channel::<bool>(10);

        let sequence = Arc::new(AtomicU16::new(0));
        let timestamp = Arc::new(AtomicU32::new(0));
        let rtp_lock = Arc::new(Mutex::new(true));

        let spatial_layer = Arc::new(AtomicU8::new(2));
        let temporal_layer = Arc::new(AtomicU8::new(2));

        tokio::spawn(
            enc!((id, media_ssrc, track_local, rtp_sender, replaced_sender, publisher_rtcp_sender, rtp_lock, sequence, timestamp, spatial_layer, temporal_layer) async move {
                Self::rtp_event_loop(
                    id,
                    media_ssrc,
                    track_local,
                    rtp_sender,
                    replaced_sender,
                    publisher_rtcp_sender,
                    rtp_lock,
                    sequence,
                    timestamp,
                    spatial_layer,
                    temporal_layer,
                )
                .await;
            }),
        );

        let rtcp_lock = Arc::new(Mutex::new(true));
        let (event_sender, event_receiver) = mpsc::unbounded_channel::<SubscriberEvent>();

        tokio::spawn(
            enc!((rtcp_sender, publisher_rtcp_sender, id, media_ssrc, replaced_sender, rtcp_lock, event_sender) async move {
                Self::rtcp_event_loop(id, media_ssrc, rtcp_sender, publisher_rtcp_sender, replaced_sender, rtcp_lock, event_sender).await;
            }),
        );

        tracing::debug!(
            "Subscriber id={} is created for publisher_ssrc={}",
            id,
            media_ssrc
        );

        let subscriber = Arc::new(Mutex::new(Self {
            publisher_id,
            id: id.clone(),
            closed_sender: tx.clone(),
            replaced_sender: replaced_sender.clone(),
            router_event_sender,
            subscriber_event_sender: event_sender.clone(),
            track_local,
            publisher_rtcp_sender,
            rtcp_sender,
            sequence,
            timestamp,
            rtp_lock,
            rtcp_lock,
            media_ssrc,
            spatial_layer,
            temporal_layer,
        }));

        tokio::spawn(enc!((id, subscriber, tx) async move {
            Self::subscriber_event_loop(id, subscriber, event_receiver, tx).await;
        }));

        (subscriber, event_sender)
    }

    pub async fn set_preferred_layer(
        &mut self,
        spatial_layer: u8,
        temporal_layer: Option<u8>,
    ) -> Result<(), Error> {
        tracing::debug!(
            "set_preferred_layer, sid={:#?}, tid={:#?}",
            spatial_layer,
            temporal_layer
        );
        #[allow(unused)]
        let mut publisher_type = PublisherType::Simple;
        {
            match Router::find_publisher(
                self.router_event_sender.clone(),
                self.publisher_id.clone(),
            )
            .await
            {
                Ok(publisher) => {
                    let guard = publisher.lock().await;
                    publisher_type = guard.publisher_type.clone();
                }
                Err(_) => {
                    let publisher = Router::find_relayed_publisher(
                        self.router_event_sender.clone(),
                        self.publisher_id.clone(),
                    )
                    .await?;
                    let guard = publisher.lock().await;
                    publisher_type = guard.publisher_type.clone();
                }
            }
        }
        if publisher_type == PublisherType::Simulcast {
            self.change_rid(spatial_layer.into()).await?;
        } else {
            let sid = self.spatial_layer.clone();
            sid.store(spatial_layer, Ordering::Relaxed);

            if let Some(temporal_layer) = temporal_layer {
                let tid = self.temporal_layer.clone();
                tid.store(temporal_layer, Ordering::Relaxed);
            }
        }

        Ok(())
    }

    async fn find_local_track(&self, rid: RID) -> Result<Arc<dyn Track>, Error> {
        match Router::find_local_track(
            self.router_event_sender.clone(),
            self.publisher_id.clone(),
            rid.clone(),
        )
        .await
        {
            Ok(track) => Ok(track),
            Err(_) => {
                match Router::find_relayed_track(
                    self.router_event_sender.clone(),
                    self.publisher_id.clone(),
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

    async fn change_rid(&mut self, rid: RID) -> Result<(), Error> {
        tracing::debug!("change_rid: {}", rid);
        let local_track = self.find_local_track(rid).await?;

        if local_track.ssrc() == self.media_ssrc {
            tracing::debug!("rid does not change");
            return Ok(());
        }
        self.media_ssrc = local_track.ssrc();

        {
            let id = self.id.clone();
            let ssrc = local_track.ssrc();
            let track_local = self.track_local.clone();
            let rtp_sender = local_track.rtp_packet_sender();
            let replaced_sender = self.replaced_sender.clone();
            let publisher_rtcp_sender = self.publisher_rtcp_sender.clone();
            let rtp_lock = self.rtp_lock.clone();
            let sequence = self.sequence.clone();
            let timestamp = self.timestamp.clone();
            let spatial_layer = self.spatial_layer.clone();
            let temporal_layer = self.temporal_layer.clone();
            tokio::spawn(async move {
                Self::rtp_event_loop(
                    id,
                    ssrc,
                    track_local,
                    rtp_sender,
                    replaced_sender,
                    publisher_rtcp_sender,
                    rtp_lock,
                    sequence,
                    timestamp,
                    spatial_layer,
                    temporal_layer,
                )
                .await;
            });
        }
        {
            let id = self.id.clone();
            let ssrc = local_track.ssrc();
            let rtcp_sender = self.rtcp_sender.clone();
            let replaced_sender = self.replaced_sender.clone();
            let publisher_rtcp_sender = self.publisher_rtcp_sender.clone();
            let loop_lock = self.rtcp_lock.clone();
            let event_sender = self.subscriber_event_sender.clone();
            tokio::spawn(async move {
                Self::rtcp_event_loop(
                    id,
                    ssrc,
                    rtcp_sender,
                    publisher_rtcp_sender,
                    replaced_sender,
                    loop_lock,
                    event_sender,
                )
                .await;
            });
        }

        if let Err(err) = self.replaced_sender.send(true) {
            tracing::error!("Failed to send replaced: {}", err);
        }

        Ok(())
    }

    pub(crate) async fn rtp_event_loop(
        id: String,
        media_ssrc: u32,
        track_local: Arc<TrackLocalStaticRTP>,
        rtp_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
        replaced_sender: broadcast::Sender<bool>,
        publisher_rtcp_sender: Arc<transport::RtcpSender>,
        loop_lock: Arc<Mutex<bool>>,
        init_sequence: Arc<AtomicU16>,
        init_timestamp: Arc<AtomicU32>,
        spatial_layer: Arc<AtomicU8>,
        temporal_layer: Arc<AtomicU8>,
    ) {
        let mut _gurad = loop_lock.lock().await;

        let mut rtp_receiver = rtp_sender.subscribe();
        drop(rtp_sender);
        let mut track_replaced = replaced_sender.subscribe();

        tracing::debug!(
            "Subscriber id={} publisher_ssrc={} RTP event loop has started",
            id,
            media_ssrc
        );

        let mut current_timestamp = init_timestamp.load(Ordering::Relaxed);
        let mut last_sequence_number: u16 = init_sequence.load(Ordering::Relaxed);

        let mut pending_packet: Option<rtp::packet::Packet> = None;

        loop {
            tokio::select! {
                _ = track_replaced.recv() => {
                    break;
                }
                res = rtp_receiver.recv() => {
                    if publisher_rtcp_sender.is_closed() {
                        break;
                    }
                    match res {
                        Ok((mut packet, layer)) => {
                            let tid = temporal_layer.load(Ordering::Relaxed);
                            let sid = spatial_layer.load(Ordering::Relaxed);

                            if layer.temporal_id > tid || layer.spatial_id > sid {
                                // In SVC, RTP marker bit that indicates end of frame is only set on the last packet of the highest spatial layer.
                                // When we filter out higher layers, we also drop the packet with marker=true.
                                // As a result, the decoder cannot detect frame boundaries correctly.
                                // To fix this, we buffer one packet and set marker=true on the last packet of the target spatial layer when dropping a packet that has marker=true.
                                if packet.header.marker {
                                    if let Some(mut pending) = pending_packet.take() {
                                        pending.header.marker = true;
                                        if let Err(err) = track_local.write_rtp(&pending).await {
                                            tracing::error!("Subscriber id={} failed to write rtp: {}", id, err)
                                        }
                                    }
                                }
                                continue
                            }

                            if let Some(pending) = pending_packet.take() {
                                if let Err(err) = track_local.write_rtp(&pending).await {
                                    tracing::error!("Subscriber id={} failed to write rtp: {}", id, err)
                                }
                            }

                            current_timestamp += packet.header.timestamp;
                            packet.header.timestamp = current_timestamp;
                            last_sequence_number = last_sequence_number.wrapping_add(1);
                            packet.header.sequence_number = last_sequence_number;

                            tracing::trace!(
                                "Subscriber id={} write RTP ssrc={} seq={} timestamp={}",
                                id,
                                packet.header.ssrc,
                                packet.header.sequence_number,
                                packet.header.timestamp
                            );

                            pending_packet = Some(packet);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            break;
                        }
                        Err(err) => {
                            tracing::error!("Subscriber id={} failed to read rtp: {}", id, err);
                        }
                    }
                }
            }
        }

        if let Some(pending) = pending_packet.take() {
            let _ = track_local.write_rtp(&pending).await;
        }

        init_sequence.store(last_sequence_number, Ordering::Relaxed);
        init_timestamp.store(current_timestamp, Ordering::Relaxed);

        tracing::debug!(
            "Subscriber id={} publisher_ssrc={} RTP event loop has finished",
            id,
            media_ssrc
        );
    }

    pub(crate) async fn rtcp_event_loop(
        id: String,
        media_ssrc: u32,
        rtcp_sender: Arc<RTCRtpSender>,
        publisher_rtcp_sender: Arc<transport::RtcpSender>,
        replaced_sender: broadcast::Sender<bool>,
        loop_lock: Arc<Mutex<bool>>,
        event_sender: mpsc::UnboundedSender<SubscriberEvent>,
    ) {
        let mut _guard = loop_lock.lock().await;

        let mut track_replaced = replaced_sender.subscribe();

        tracing::debug!(
            "Subscriber id={} publisher_ssrc={} RTCP event loop has started",
            id,
            media_ssrc
        );

        loop {
            tokio::select! {
                _ = track_replaced.recv() => {
                    break;
                }
                res = rtcp_sender.read_rtcp() => {
                    if publisher_rtcp_sender.is_closed() {
                        if let Err(err) = event_sender.send(SubscriberEvent::Close) {
                            tracing::error!("Failed to send subscriber close event: {}", err);
                        }
                        break;
                    }
                    match res {
                        Ok((rtcp_packets, attr)) => {
                            for rtcp in rtcp_packets.into_iter() {
                                tracing::trace!("Receive RTCP subscriber={} rtcp={:#?}, attr={:#?}", id, rtcp, attr);

                                let header = rtcp.header();
                                match header.packet_type {
                                    PacketType::ReceiverReport => {
                                        if let Some(_rr) = rtcp
                                            .as_any()
                                            .downcast_ref::<rtcp::receiver_report::ReceiverReport>()
                                        {
                                            // Received receiver reports.
                                        }
                                    }
                                    PacketType::PayloadSpecificFeedback => match header.count {
                                        FORMAT_PLI => {
                                            if let Some(_pli) = rtcp.as_any().downcast_ref::<rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>() {
                                                match publisher_rtcp_sender.send(Box::new(PictureLossIndication {
                                                    sender_ssrc: 0,
                                                    media_ssrc,
                                                })) {
                                                    Ok(_) => tracing::trace!("send rtcp: pli"),
                                                    Err(err) => tracing::error!("Subscriber id ={} failed to send rtcp pli: {}", id, err)
                                                }
                                            }
                                        }
                                        _ => {}
                                    },
                                    _ => {}
                                }
                            }

                        }
                        Err(webrtc::error::Error::ErrDataChannelNotOpen) => {
                            break;
                        }
                        Err(webrtc::error::Error::ErrClosedPipe) => {
                            if let Err(err) = event_sender.send(SubscriberEvent::Close) {
                                tracing::error!("Failed to send subscriber close event: {}", err);
                            }
                            break;
                        }
                        Err(err) => {
                            tracing::error!("Subscriber id={} failed to read rtcp: {:#?}", id, err);
                        }
                    }
                }
            }
        }

        tracing::debug!(
            "Subscriber id={} publisher_ssrc={} RTCP event loop finished",
            id,
            media_ssrc
        );
    }

    pub(crate) async fn subscriber_event_loop(
        id: String,
        subscriber: Arc<Mutex<Subscriber>>,
        mut event_receiver: mpsc::UnboundedReceiver<SubscriberEvent>,
        subscriber_closed_sender: broadcast::Sender<bool>,
    ) {
        let mut subscriber_closed = subscriber_closed_sender.subscribe();

        loop {
            tokio::select! {
                _ = subscriber_closed.recv() => {
                    break;
                }
                Some(event) = event_receiver.recv() => {
                    match event {
                        SubscriberEvent::SetPrefferedLayer(sid, tid) => {
                            let mut guard = subscriber.lock().await;
                            if let Err(err) = guard.set_preferred_layer(sid, tid).await {
                                tracing::error!("Failed to set preferred layer: {}", err);
                            }
                        }
                        SubscriberEvent::Close => {
                            subscriber_closed_sender.send(true).unwrap();
                            break;
                        }
                    }
                }
            }
        }

        tracing::debug!("Subscriber {} event loop finished", id);
    }

    pub async fn close(&self) {
        tracing::debug!("Subscriber id={} is closed", self.id);
        let _ = self.closed_sender.send(true);
        let _ = self.replaced_sender.send(true);
    }
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        tracing::debug!("Subscriber id={} is dropped", self.id);
        let _ = self.closed_sender.send(true);
        let _ = self.replaced_sender.send(true);
    }
}

pub(crate) enum SubscriberEvent {
    SetPrefferedLayer(u8, Option<u8>),
    Close,
}

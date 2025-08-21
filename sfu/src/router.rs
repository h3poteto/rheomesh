use std::{collections::HashMap, sync::Arc};

use crate::{
    config::{MediaConfig, RID, WebRTCTransportConfig},
    data_publisher::DataPublisher,
    error::{Error, SubscriberErrorKind},
    local_track::LocalTrack,
    publish_transport::PublishTransport,
    publisher::Publisher,
    recording::recording_transport::RecordingTransport,
    relay::{
        relayed_data_publisher::RelayedDataPublisher, relayed_publisher::RelayedPublisher,
        relayed_track::RelayedTrack, sender::RelaySender,
    },
    subscribe_transport::SubscribeTransport,
    worker::WorkerEvent,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use uuid::Uuid;

/// Router accommodates multiple transports and they can communicate with each other. That means transports belonging to the same Router can send/receive their media. Router is like a meeting room.
#[derive(Debug)]
pub struct Router {
    pub id: String,
    publishers: Vec<(String, Arc<Mutex<Publisher>>)>,
    relayed_publishers: Vec<(String, Arc<Mutex<RelayedPublisher>>)>,
    data_publishers: HashMap<String, Arc<Mutex<DataPublisher>>>,
    relayed_data_publishers: Vec<(String, Arc<Mutex<RelayedDataPublisher>>)>,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    media_config: MediaConfig,
    relay_sender: Arc<RelaySender>,
    worker_event_sender: mpsc::UnboundedSender<WorkerEvent>,
    private_ip: String,
}

impl Router {
    pub(crate) fn new(
        media_config: MediaConfig,
        relay_sender: Arc<RelaySender>,
        worker_event_sender: mpsc::UnboundedSender<WorkerEvent>,
        private_ip: String,
    ) -> (Arc<Mutex<Router>>, String) {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::unbounded_channel::<RouterEvent>();

        let r = Router {
            id: id.clone(),
            publishers: Vec::new(),
            relayed_publishers: Vec::new(),
            data_publishers: HashMap::new(),
            relayed_data_publishers: Vec::new(),
            router_event_sender: tx,
            media_config,
            relay_sender,
            worker_event_sender,
            private_ip,
        };

        tracing::debug!("Router {} is created", id);

        let router = Arc::new(Mutex::new(r));
        {
            let copied = Arc::clone(&router);
            let id = id.clone();
            tokio::spawn(async move {
                Router::router_event_loop(id, copied, rx).await;
            });
        }

        (router, id)
    }

    /// This returns [`crate::publisher::Publisher`] IDs that has already been published in this router. It is useful when a new user connect to the router and get already published media.
    pub fn publisher_ids(&self) -> Vec<String> {
        self.publishers
            .clone()
            .into_iter()
            .map(|(k, _)| k)
            .collect()
    }

    /// This returns [`crate::data_publisher::DataPublisher`] IDs that has already been published in this router. It is useful when a new user connect to the router and get already published data channels.
    pub fn data_publisher_ids(&self) -> Vec<String> {
        self.data_publishers
            .clone()
            .into_iter()
            .map(|(k, _)| k)
            .collect()
    }

    pub async fn create_publish_transport(
        &self,
        transport_config: WebRTCTransportConfig,
    ) -> PublishTransport {
        let tx = self.router_event_sender.clone();
        PublishTransport::new(
            tx,
            self.media_config.clone(),
            transport_config,
            self.relay_sender.clone(),
            self.private_ip.clone(),
        )
        .await
    }

    pub async fn create_subscribe_transport(
        &self,
        transport_config: WebRTCTransportConfig,
    ) -> SubscribeTransport {
        let tx = self.router_event_sender.clone();
        SubscribeTransport::new(tx, self.media_config.clone(), transport_config).await
    }

    pub async fn create_recording_transport(
        &self,
        ip: String,
        port: u16,
    ) -> Result<RecordingTransport, Error> {
        let tx = self.router_event_sender.clone();
        RecordingTransport::new(ip, port, tx).await
    }

    pub(crate) async fn router_event_loop(
        id: String,
        router: Arc<Mutex<Router>>,
        mut event_receiver: mpsc::UnboundedReceiver<RouterEvent>,
    ) {
        tracing::debug!("Router {} event loop started", id);
        while let Some(event) = event_receiver.recv().await {
            match event {
                RouterEvent::MediaPublished(track_id, publisher) => {
                    let mut r = router.lock().await;
                    r.publishers.push((track_id, publisher));
                }
                RouterEvent::PublisherRemoved(track_id) => {
                    let mut r = router.lock().await;
                    r.publishers.retain(|(id, _)| *id != track_id);
                }
                RouterEvent::GetPublisher(track_id, reply_sender) => {
                    let r = router.lock().await;
                    let track = r
                        .publishers
                        .iter()
                        .find(|(id, _)| *id == track_id)
                        .map(|(_, publisher)| publisher);
                    let data = track.cloned();
                    let _ = reply_sender.send(data);
                }
                RouterEvent::GetRelayedPublisher(track_id, reply_sender) => {
                    let r = router.lock().await;
                    let publisher = r
                        .relayed_publishers
                        .iter()
                        .find(|(id, _)| *id == track_id)
                        .map(|(_, publisher)| publisher);
                    let data = publisher.cloned();
                    let _ = reply_sender.send(data);
                }
                RouterEvent::DataPublished(data_publisher) => {
                    let mut r = router.lock().await;
                    #[allow(unused)]
                    let mut data_id = "".to_string();
                    {
                        let data_publisher = data_publisher.lock().await;
                        data_id = data_publisher.id.clone();
                    }
                    r.data_publishers.insert(data_id, data_publisher);
                }
                RouterEvent::DataRemoved(data_publisher_id) => {
                    let mut r = router.lock().await;
                    r.data_publishers.remove(&data_publisher_id);
                }
                RouterEvent::GetDataPublisher(data_publisher_id, reply_sender) => {
                    let r = router.lock().await;
                    let channel = r.data_publishers.get(&data_publisher_id);
                    let data = channel.cloned();
                    let _ = reply_sender.send(data);
                }
                RouterEvent::GetRelayedDataPublisher(data_publisher_id, reply_sender) => {
                    let r = router.lock().await;
                    let data_publisher = r
                        .relayed_data_publishers
                        .iter()
                        .find(|(id, _)| *id == data_publisher_id)
                        .map(|(_, publisher)| publisher);
                    let data = data_publisher.cloned();
                    let _ = reply_sender.send(data);
                }
                RouterEvent::Closed => {
                    let r = router.lock().await;
                    if let Err(err) = r
                        .worker_event_sender
                        .send(WorkerEvent::RouterRemoved(r.id.clone()))
                    {
                        tracing::error!("Failed to send RouterRemoved event id={}: {}", r.id, err);
                    }
                    break;
                }
            }
        }
        tracing::debug!("Router {} event loop finished", id);
    }

    pub(crate) async fn find_publisher(
        event_sender: mpsc::UnboundedSender<RouterEvent>,
        publisher_id: String,
    ) -> Result<Arc<Mutex<Publisher>>, Error> {
        let (tx, rx) = oneshot::channel();

        let _ = event_sender.send(RouterEvent::GetPublisher(publisher_id.clone(), tx));

        let reply = rx.await.unwrap();
        match reply {
            None => {
                return Err(Error::new_subscriber(
                    format!("Publisher for {} is not found", publisher_id),
                    SubscriberErrorKind::TrackNotFoundError,
                ));
            }
            Some(publisher) => Ok(publisher),
        }
    }

    pub(crate) async fn find_local_track(
        event_sender: mpsc::UnboundedSender<RouterEvent>,
        publisher_id: String,
        rid: RID,
    ) -> Result<Arc<LocalTrack>, Error> {
        let publisher = Self::find_publisher(event_sender, publisher_id).await?;

        let guard = publisher.lock().await;
        let local_track = guard.get_local_track(rid.to_string().as_str())?;
        Ok(local_track)
    }

    pub(crate) async fn find_local_data_publisher(
        event_sender: mpsc::UnboundedSender<RouterEvent>,
        data_publisher_id: String,
    ) -> Result<Arc<Mutex<DataPublisher>>, Error> {
        let (tx, rx) = oneshot::channel();

        let _ = event_sender.send(RouterEvent::GetDataPublisher(data_publisher_id.clone(), tx));

        let reply = rx.await.unwrap();
        match reply {
            None => {
                return Err(Error::new_subscriber(
                    format!("DataPublisher for {} is not found", data_publisher_id),
                    SubscriberErrorKind::TrackNotFoundError,
                ));
            }
            Some(data_publisher) => Ok(data_publisher),
        }
    }

    pub(crate) async fn add_relayed_publisher(
        &mut self,
        track_id: String,
        relayed_publisher: Arc<Mutex<RelayedPublisher>>,
    ) {
        self.relayed_publishers.push((track_id, relayed_publisher));
    }

    pub(crate) async fn add_relayed_data_publisher(
        &mut self,
        data_publisher_id: String,
        relayed_data_publisher: Arc<Mutex<RelayedDataPublisher>>,
    ) {
        self.relayed_data_publishers
            .push((data_publisher_id, relayed_data_publisher));
    }

    pub(crate) async fn find_relayed_publisher(
        event_sender: mpsc::UnboundedSender<RouterEvent>,
        publisher_id: String,
    ) -> Result<Arc<Mutex<RelayedPublisher>>, Error> {
        let (tx, rx) = oneshot::channel();

        let _ = event_sender.send(RouterEvent::GetRelayedPublisher(publisher_id.clone(), tx));

        let reply = rx.await.unwrap();
        match reply {
            None => {
                return Err(Error::new_subscriber(
                    format!("RelayedPublisher for {} is not found", publisher_id),
                    SubscriberErrorKind::TrackNotFoundError,
                ));
            }
            Some(publisher) => Ok(publisher),
        }
    }

    pub(crate) async fn find_relayed_data_publisher(
        event_sender: mpsc::UnboundedSender<RouterEvent>,
        data_publisher_id: String,
    ) -> Result<Arc<Mutex<RelayedDataPublisher>>, Error> {
        let (tx, rx) = oneshot::channel();

        let _ = event_sender.send(RouterEvent::GetRelayedDataPublisher(
            data_publisher_id.clone(),
            tx,
        ));

        let reply = rx.await.unwrap();
        match reply {
            None => {
                return Err(Error::new_subscriber(
                    format!(
                        "RelayedDataPublisher for {} is not found",
                        data_publisher_id
                    ),
                    SubscriberErrorKind::TrackNotFoundError,
                ));
            }
            Some(data_publisher) => Ok(data_publisher),
        }
    }

    pub(crate) async fn find_relayed_track(
        event_sender: mpsc::UnboundedSender<RouterEvent>,
        publisher_id: String,
        rid: RID,
    ) -> Result<Arc<RelayedTrack>, Error> {
        let publisher = Self::find_relayed_publisher(event_sender, publisher_id).await?;

        let guard = publisher.lock().await;
        let relayed_track = guard.get_relayed_track(rid.to_string().as_str())?;
        Ok(relayed_track)
    }

    pub(crate) async fn remove_relayed_publisher(&mut self, track_id: &str) {
        self.relayed_publishers.retain(|(id, _)| id != track_id);
    }

    pub(crate) async fn remove_relayed_data_publisher(&mut self, data_publisher_id: &str) {
        self.relayed_data_publishers
            .retain(|(id, _)| id != data_publisher_id);
    }

    pub fn close(&self) {
        let _ = self.router_event_sender.send(RouterEvent::Closed);
    }
}

#[derive(Debug)]
pub(crate) enum RouterEvent {
    MediaPublished(String, Arc<Mutex<Publisher>>),
    PublisherRemoved(String),
    DataPublished(Arc<Mutex<DataPublisher>>),
    DataRemoved(String),
    GetPublisher(String, oneshot::Sender<Option<Arc<Mutex<Publisher>>>>),
    GetDataPublisher(String, oneshot::Sender<Option<Arc<Mutex<DataPublisher>>>>),
    GetRelayedPublisher(
        String,
        oneshot::Sender<Option<Arc<Mutex<RelayedPublisher>>>>,
    ),
    GetRelayedDataPublisher(
        String,
        oneshot::Sender<Option<Arc<Mutex<RelayedDataPublisher>>>>,
    ),
    Closed,
}

impl Drop for Router {
    fn drop(&mut self) {
        tracing::debug!("Router {} is dropped", self.id);
    }
}

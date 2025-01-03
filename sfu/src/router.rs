use std::{collections::HashMap, sync::Arc};

use crate::{
    config::{MediaConfig, WebRTCTransportConfig, RID},
    data_publisher::DataPublisher,
    error::{Error, SubscriberErrorKind},
    local_track::LocalTrack,
    publish_transport::PublishTransport,
    publisher::Publisher,
    subscribe_transport::SubscribeTransport,
};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use uuid::Uuid;

/// Router accommodates multiple transports and they can communicate with each other. That means transports belonging to the same Router can send/receive their media. Router is like a meeting room.
#[derive(Debug)]
pub struct Router {
    pub id: String,
    publishers: Vec<(String, Arc<RwLock<Publisher>>)>,
    data_publishers: HashMap<String, Arc<DataPublisher>>,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
    media_config: MediaConfig,
}

impl Router {
    pub fn new(media_config: MediaConfig) -> Arc<Mutex<Router>> {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::unbounded_channel::<RouterEvent>();

        let r = Router {
            id: id.clone(),
            publishers: Vec::new(),
            data_publishers: HashMap::new(),
            router_event_sender: tx,
            media_config,
        };

        tracing::debug!("Router {} is created", id);

        let router = Arc::new(Mutex::new(r));
        let copied = Arc::clone(&router);
        tokio::spawn(async move {
            Router::router_event_loop(id, copied, rx).await;
        });

        router
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
        PublishTransport::new(tx, self.media_config.clone(), transport_config).await
    }

    pub async fn create_subscribe_transport(
        &self,
        transport_config: WebRTCTransportConfig,
    ) -> SubscribeTransport {
        let tx = self.router_event_sender.clone();
        SubscribeTransport::new(tx, self.media_config.clone(), transport_config).await
    }

    pub(crate) async fn router_event_loop(
        id: String,
        router: Arc<Mutex<Router>>,
        mut event_receiver: mpsc::UnboundedReceiver<RouterEvent>,
    ) {
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
                RouterEvent::DataPublished(data_publisher) => {
                    let mut r = router.lock().await;
                    let data_id = data_publisher.id.clone();
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
                RouterEvent::Closed => {
                    break;
                }
            }
        }
        tracing::debug!("Router {} event loop finished", id);
    }

    pub(crate) async fn find_local_track(
        event_sender: mpsc::UnboundedSender<RouterEvent>,
        publisher_id: String,
        rid: RID,
    ) -> Result<Arc<LocalTrack>, Error> {
        let (tx, rx) = oneshot::channel();

        let _ = event_sender.send(RouterEvent::GetPublisher(publisher_id.clone(), tx));

        let reply = rx.await.unwrap();
        match reply {
            None => {
                return Err(Error::new_subscriber(
                    format!("Publisher for {} is not found", publisher_id),
                    SubscriberErrorKind::TrackNotFoundError,
                ))
            }
            Some(publisher) => {
                let guard = publisher.read().await;
                let local_track = guard.get_local_track(rid.to_string().as_str())?;
                Ok(local_track)
            }
        }
    }

    pub fn close(&self) {
        let _ = self.router_event_sender.send(RouterEvent::Closed);
    }
}

#[derive(Debug)]
pub(crate) enum RouterEvent {
    MediaPublished(String, Arc<RwLock<Publisher>>),
    PublisherRemoved(String),
    DataPublished(Arc<DataPublisher>),
    DataRemoved(String),
    GetPublisher(String, oneshot::Sender<Option<Arc<RwLock<Publisher>>>>),
    GetDataPublisher(String, oneshot::Sender<Option<Arc<DataPublisher>>>),
    Closed,
}

impl Drop for Router {
    fn drop(&mut self) {
        tracing::debug!("Router {} is dropped", self.id);
    }
}

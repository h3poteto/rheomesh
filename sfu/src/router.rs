use std::{collections::HashMap, sync::Arc};

use crate::{
    config::{MediaConfig, WebRTCTransportConfig},
    data_publisher::DataPublisher,
    publish_transport::PublishTransport,
    publisher::Publisher,
    subscribe_transport::SubscribeTransport,
};
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

#[derive(Clone)]
pub struct Router {
    pub id: String,
    publishers: HashMap<String, Arc<Publisher>>,
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
            publishers: HashMap::new(),
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

    pub fn track_ids(&self) -> Vec<String> {
        self.publishers
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
    ) -> Arc<SubscribeTransport> {
        let tx = self.router_event_sender.clone();
        SubscribeTransport::new(tx, self.media_config.clone(), transport_config).await
    }

    pub async fn router_event_loop(
        id: String,
        router: Arc<Mutex<Router>>,
        mut event_receiver: mpsc::UnboundedReceiver<RouterEvent>,
    ) {
        while let Some(event) = event_receiver.recv().await {
            match event {
                RouterEvent::TrackPublished(publisher) => {
                    let mut r = router.lock().await;
                    let track_id = publisher.id.clone();
                    r.publishers.insert(track_id, publisher);
                }
                RouterEvent::TrackRemoved(track_id) => {
                    let mut r = router.lock().await;
                    r.publishers.remove(&track_id);
                }
                RouterEvent::GetPublisher(track_id, reply_sender) => {
                    let r = router.lock().await;
                    let track = r.publishers.get(&track_id);
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

    pub fn close(&self) {
        let _ = self.router_event_sender.send(RouterEvent::Closed);
    }
}

pub enum RouterEvent {
    TrackPublished(Arc<Publisher>),
    TrackRemoved(String),
    DataPublished(Arc<DataPublisher>),
    DataRemoved(String),
    GetPublisher(String, oneshot::Sender<Option<Arc<Publisher>>>),
    GetDataPublisher(String, oneshot::Sender<Option<Arc<DataPublisher>>>),
    Closed,
}

impl Drop for Router {
    fn drop(&mut self) {
        tracing::debug!("Router {} is dropped", self.id);
    }
}

use std::{collections::HashMap, sync::Arc};

use crate::{
    config::WebRTCTransportConfig, data_track::DataTrack, media_track::MediaTrack,
    subscriber::Subscriber, transport::Transport,
};
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

#[derive(Clone)]
pub struct Router {
    pub id: String,
    media_tracks: HashMap<String, Arc<MediaTrack>>,
    data_tracks: HashMap<u16, Arc<DataTrack>>,
    subscribers: HashMap<String, Arc<Subscriber>>,
    router_event_sender: mpsc::UnboundedSender<RouterEvent>,
}

impl Router {
    pub fn new() -> Arc<Mutex<Router>> {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::unbounded_channel::<RouterEvent>();

        let r = Router {
            id: id.clone(),
            media_tracks: HashMap::new(),
            data_tracks: HashMap::new(),
            subscribers: HashMap::new(),
            router_event_sender: tx,
        };

        let router = Arc::new(Mutex::new(r));
        let copied = Arc::clone(&router);
        tokio::spawn(async move {
            Router::router_event_loop(id, copied, rx).await;
        });

        router
    }

    pub fn track_ids(&self) -> Vec<String> {
        self.media_tracks
            .clone()
            .into_iter()
            .map(|(k, _)| k)
            .collect()
    }

    pub async fn create_transport(&self, transport_config: WebRTCTransportConfig) -> Transport {
        let tx = self.router_event_sender.clone();
        Transport::new(tx, transport_config).await
    }

    pub async fn router_event_loop(
        id: String,
        router: Arc<Mutex<Router>>,
        mut event_receiver: mpsc::UnboundedReceiver<RouterEvent>,
    ) {
        while let Some(event) = event_receiver.recv().await {
            match event {
                RouterEvent::TrackPublished(track) => {
                    let mut r = router.lock().await;
                    let track_id = track.id.clone();
                    r.media_tracks.insert(track_id, Arc::new(track));
                }
                RouterEvent::TrackRemoved(track_id) => {
                    let mut r = router.lock().await;
                    r.media_tracks.remove(&track_id);
                }
                RouterEvent::DataChannelOpened(data_track) => {
                    let mut r = router.lock().await;
                    let data_channel_id = data_track.id.clone();
                    r.data_tracks.insert(data_channel_id, Arc::new(data_track));
                }
                RouterEvent::DataChannelClosed(data_channel_id) => {
                    let mut r = router.lock().await;
                    r.data_tracks.remove(&data_channel_id);
                }
                RouterEvent::SubscriberAdded(subscriber) => {
                    let mut r = router.lock().await;
                    let subscriber_id = subscriber.id.clone();
                    r.subscribers.insert(subscriber_id, subscriber);
                }
                RouterEvent::SubscriberRemoved(subscriber_id) => {
                    let mut r = router.lock().await;
                    r.subscribers.remove(&subscriber_id);
                }
                RouterEvent::GetMediaTrack(track_id, reply_sender) => {
                    let r = router.lock().await;
                    let track = r.media_tracks.get(&track_id);
                    let data = track.cloned();
                    let _ = reply_sender.send(data);
                }
                RouterEvent::GetDataTrack(data_channel_id, reply_sender) => {
                    let r = router.lock().await;
                    let data = r.data_tracks.get(&data_channel_id);
                    let data = data.cloned();
                    let _ = reply_sender.send(data);
                }
                RouterEvent::Closed => {
                    break;
                }
            }
        }
        tracing::info!("Router {} event loop finished", id);
    }

    pub fn close(&self) {
        let _ = self.router_event_sender.send(RouterEvent::Closed);
    }
}

pub enum RouterEvent {
    TrackPublished(MediaTrack),
    TrackRemoved(String),
    DataChannelOpened(DataTrack),
    DataChannelClosed(u16),
    SubscriberAdded(Arc<Subscriber>),
    SubscriberRemoved(String),
    GetMediaTrack(String, oneshot::Sender<Option<Arc<MediaTrack>>>),
    GetDataTrack(u16, oneshot::Sender<Option<Arc<DataTrack>>>),
    Closed,
}

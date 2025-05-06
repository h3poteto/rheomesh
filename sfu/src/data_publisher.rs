use std::sync::Arc;

use derivative::Derivative;
use enclose::enc;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;
use webrtc::data_channel::{data_channel_message::DataChannelMessage, RTCDataChannel};

use crate::router::RouterEvent;

#[derive(Derivative)]
#[derivative(Debug)]
pub struct DataPublisher {
    pub id: String,
    pub channel_id: u16,
    pub label: String,
    pub(crate) data_sender: broadcast::Sender<DataChannelMessage>,
    #[derivative(Debug = "ignore")]
    data_channel: Arc<RTCDataChannel>,
}

impl DataPublisher {
    pub(crate) fn new(
        data_channel: Arc<RTCDataChannel>,
        router_sender: mpsc::UnboundedSender<RouterEvent>,
    ) -> Self {
        let channel_id = data_channel.id();
        let label = data_channel.label().to_string();

        let id = Uuid::new_v4().to_string();
        let cloned_id = id.clone();
        data_channel.on_close(Box::new(enc!((router_sender, cloned_id) move || {
            tracing::debug!("DataChannel {} has been closed", cloned_id);
            Box::pin(enc!((router_sender, cloned_id) async move {
                let _ = router_sender.send(RouterEvent::DataRemoved(cloned_id));
            }))
        })));

        data_channel.on_error(Box::new(move |err| {
            Box::pin(async move {
                tracing::debug!("Error on DataChannel: {}", err);
            })
        }));

        let (data_sender, _data_receiver) = broadcast::channel(1024);
        let sender = data_sender.clone();
        data_channel.on_message(Box::new(move |msg| {
            tracing::debug!("message: {:#?}", msg.data);
            let data_sender = sender.clone();
            Box::pin(async move {
                let _ = data_sender.send(msg);
            })
        }));

        tracing::debug!("DataPublisher {} is created, label={}", id, label);

        let publisher = Self {
            id,
            channel_id,
            label,
            data_sender,
            data_channel,
        };

        publisher
    }

    pub async fn close(&self) {
        tracing::debug!("DataPublisher is closed");
        let _ = self.data_channel.close().await;
    }
}

impl Drop for DataPublisher {
    fn drop(&mut self) {
        tracing::debug!("DataPublisher {} is dropped", self.id);
    }
}

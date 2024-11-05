use std::sync::Arc;

use tokio::sync::{broadcast, oneshot};
use webrtc::data_channel::{data_channel_message::DataChannelMessage, RTCDataChannel};

#[derive(Clone, Debug)]
pub struct DataPublisher {
    pub id: String,
    pub data_sender: broadcast::Sender<DataChannelMessage>,
}

impl DataPublisher {
    pub fn new(data_channel: Arc<RTCDataChannel>) -> (Arc<Self>, oneshot::Receiver<bool>) {
        let id = data_channel.id();

        let (finished_sender, finished_receiver) = oneshot::channel();
        let mut finished_sender = Some(finished_sender);
        data_channel.on_close(Box::new(move || {
            let finished_sender = finished_sender.take();
            Box::pin(async move {
                if let Some(finished_sender) = finished_sender {
                    let _ = finished_sender.send(true);
                }
            })
        }));

        let (data_sender, _data_receiver) = broadcast::channel(1024);
        let sender = data_sender.clone();
        data_channel.on_message(Box::new(move |msg| {
            let data_sender = sender.clone();
            Box::pin(async move {
                let _ = data_sender.send(msg);
            })
        }));

        tracing::debug!("DataPublisher {} is created", id);

        let publisher = Self {
            id: id.to_string(),
            data_sender,
        };

        (Arc::new(publisher), finished_receiver)
    }

    pub async fn close(&self) {}
}

impl Drop for DataPublisher {
    fn drop(&mut self) {
        tracing::debug!("DataPublisher {} is dropped", self.id);
    }
}

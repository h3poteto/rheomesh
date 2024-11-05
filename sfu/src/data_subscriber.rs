use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, Mutex};
use uuid::Uuid;
use webrtc::data_channel::{
    data_channel_message::DataChannelMessage, data_channel_state::RTCDataChannelState,
    RTCDataChannel,
};

pub struct DataSubscriber {
    pub id: String,
    closed_sender: Arc<mpsc::UnboundedSender<bool>>,
}

impl DataSubscriber {
    pub fn new(
        data_publisher_id: String,
        data_channel: Arc<RTCDataChannel>,
        data_sender: broadcast::Sender<DataChannelMessage>,
        transport_closed: Arc<Mutex<mpsc::UnboundedReceiver<bool>>>,
    ) -> Self {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::unbounded_channel();
        let closed_receiver = Arc::new(Mutex::new(rx));

        tokio::spawn(async move {
            let receiver = data_sender.subscribe();

            Self::data_event_loop(
                data_publisher_id,
                data_channel,
                receiver,
                transport_closed,
                closed_receiver,
            )
            .await;
        });

        Self {
            id,
            closed_sender: Arc::new(tx),
        }
    }

    pub async fn data_event_loop(
        source_channel_id: String,
        data_channel: Arc<RTCDataChannel>,
        mut data_receiver: broadcast::Receiver<DataChannelMessage>,
        transport_closed: Arc<Mutex<mpsc::UnboundedReceiver<bool>>>,
        subscriber_closed: Arc<Mutex<mpsc::UnboundedReceiver<bool>>>,
    ) {
        tracing::debug!(
            "DataSubscriber event loop has started for {}",
            source_channel_id
        );

        loop {
            let mut transport_closed = transport_closed.lock().await;
            let mut subscriber_closed = subscriber_closed.lock().await;
            tokio::select! {
                _closed = transport_closed.recv() => {
                    break;
                }
                _closed = subscriber_closed.recv() => {
                    break;
                }
                res = data_receiver.recv() => {
                    match res {
                        Ok(res) => {
                            let state = data_channel.ready_state();
                            match state {
                                RTCDataChannelState::Open => {
                                    let data = res.data;
                                    let _ = data_channel.send(&data).await;
                                }
                                _ => {
                                    tracing::warn!("Data channel is not opened, state={:?}", state);
                                }
                            }
                        }
                        Err(err) => {
                            tracing::error!("DataSubscriber failed to receive data: {}", err);
                            break;
                        }
                    }
                }
            }
        }

        tracing::debug!(
            "DataSubscriber event loop has finished for {}",
            source_channel_id
        );
    }

    pub async fn close(&self) {
        if !self.closed_sender.is_closed() {
            self.closed_sender.send(true).unwrap();
        }
    }
}

impl Drop for DataSubscriber {
    fn drop(&mut self) {
        tracing::debug!("DataSubscriber {} is dropped", self.id);
    }
}

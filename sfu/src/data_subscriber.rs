use std::sync::Arc;

use derivative::Derivative;
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use uuid::Uuid;
use webrtc::data_channel::{DataChannel, RTCDataChannelMessage, RTCDataChannelState};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct DataSubscriber {
    pub id: String,
    closed_sender: Arc<mpsc::UnboundedSender<bool>>,
    #[derivative(Debug = "ignore")]
    data_channel: Arc<dyn DataChannel>,
}

impl DataSubscriber {
    pub(crate) fn new(
        data_publisher_id: String,
        data_channel: Arc<dyn DataChannel>,
        data_sender: broadcast::Sender<RTCDataChannelMessage>,
        transport_closed: watch::Receiver<bool>,
    ) -> Self {
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::unbounded_channel();
        let closed_receiver = Arc::new(Mutex::new(rx));

        let channel = data_channel.clone();

        {
            let id = id.clone();
            tokio::spawn(async move {
                let receiver = data_sender.subscribe();
                drop(data_sender);
                Self::data_event_loop(
                    id,
                    data_publisher_id,
                    channel.clone(),
                    receiver,
                    transport_closed,
                    closed_receiver,
                )
                .await;
                let _ = channel.close().await;
            });
        }

        Self {
            id,
            closed_sender: Arc::new(tx),
            data_channel,
        }
    }

    pub(crate) async fn data_event_loop(
        id: String,
        source_channel_id: String,
        data_channel: Arc<dyn DataChannel>,
        mut data_receiver: broadcast::Receiver<RTCDataChannelMessage>,
        mut transport_closed: watch::Receiver<bool>,
        subscriber_closed: Arc<Mutex<mpsc::UnboundedReceiver<bool>>>,
    ) {
        tracing::debug!(
            "DataSubscriber {} event loop has started for {}",
            id,
            source_channel_id
        );

        loop {
            let mut subscriber_closed = subscriber_closed.lock().await;
            tokio::select! {
                _closed = transport_closed.changed() => {
                    let closed = transport_closed.borrow();
                    if *closed {
                        tracing::debug!("Transport is closed, exiting event loop for {}", id);
                        break;
                    }
                }
                _closed = subscriber_closed.recv() => {
                    break;
                }
                res = data_receiver.recv() => {
                    match res {
                        Ok(res) => {
                            let state = data_channel.ready_state().await.unwrap_or_default();
                            match state {
                                RTCDataChannelState::Open => {
                                    tracing::debug!("DataSubscriber {} received data: {:?}", id, res.data);
                                    let _ = data_channel.send(res.data).await;
                                }
                                _ => {
                                    tracing::warn!("Data channel is not opened, state={:?}", state);
                                }
                            }
                        }
                        Err(err) => {
                            tracing::error!("DataSubscriber {} failed to receive data: {}", id, err);
                            break;
                        }
                    }
                }
            }
        }

        tracing::debug!(
            "DataSubscriber {} event loop has finished for {}",
            id,
            source_channel_id
        );
    }

    pub async fn close(&self) {
        if !self.closed_sender.is_closed() {
            self.closed_sender.send(true).unwrap();
        }
        let _ = self.data_channel.close().await;
    }
}

impl Drop for DataSubscriber {
    fn drop(&mut self) {
        tracing::debug!("DataSubscriber {} is dropped", self.id);
    }
}

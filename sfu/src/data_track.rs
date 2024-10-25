use std::sync::Arc;

use tokio::sync::{broadcast, oneshot};
use webrtc::data_channel::{data_channel_message::DataChannelMessage, RTCDataChannel};

pub struct DataTrack {
    pub id: u16,
    data_channel: Arc<RTCDataChannel>,
    pub data_sender: broadcast::Sender<DataChannelMessage>,
}

impl DataTrack {
    pub fn new(dc: Arc<RTCDataChannel>) -> (Self, oneshot::Receiver<bool>) {
        let (closed_sender, closed_receiver) = oneshot::channel();
        let mut closed_sender = Some(closed_sender);
        dc.on_close(Box::new(move || {
            let closed_sender = closed_sender.take();
            Box::pin(async move {
                if let Some(closed_sender) = closed_sender {
                    let _ = closed_sender.send(true);
                }
            })
        }));

        let (data_sender, _data_receiver) = broadcast::channel(1024);
        let sender = data_sender.clone();
        dc.on_message(Box::new(move |msg| {
            let data_sender = sender.clone();
            Box::pin(async move {
                let _ = data_sender.send(msg);
            })
        }));

        (
            Self {
                id: dc.id().clone(),
                data_channel: dc,
                data_sender,
            },
            closed_receiver,
        )
    }
}

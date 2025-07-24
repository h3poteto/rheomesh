use std::sync::Arc;

use derivative::Derivative;
use enclose::enc;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;
use webrtc::data_channel::{data_channel_message::DataChannelMessage, RTCDataChannel};

use crate::{
    data_channel::Channel,
    error::Error,
    relay::sender::{RelaySender, RelayUDPSender},
    router::RouterEvent,
};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct DataPublisher {
    pub id: String,
    pub channel_id: u16,
    pub label: String,
    pub(crate) data_sender: broadcast::Sender<DataChannelMessage>,
    #[derivative(Debug = "ignore")]
    data_channel: Arc<RTCDataChannel>,
    relay_sender: Arc<RelaySender>,
    relay_udp_sender: Option<Arc<RelayUDPSender>>,
}

impl DataPublisher {
    pub(crate) fn new(
        data_channel: Arc<RTCDataChannel>,
        router_sender: mpsc::UnboundedSender<RouterEvent>,
        relay_sender: Arc<RelaySender>,
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
            relay_sender,
            relay_udp_sender: None,
        };

        publisher
    }

    pub async fn relay_to(
        &mut self,
        ip: String,
        port: u16,
        router_id: String,
    ) -> Result<bool, Error> {
        if self.relay_udp_sender.is_none() {
            self.relay_udp_sender = Some(Arc::new(RelayUDPSender::new().await?));
        }

        let udp_port = self
            .relay_sender
            .create_relay_data(
                ip.clone(),
                port,
                router_id.clone(),
                self.id.clone(),
                self.channel_id.clone(),
                self.label.clone(),
            )
            .await?;

        let data_sender = self.data_sender.clone();
        let id = self.id.clone();
        let relay_udp_sender = self.relay_udp_sender.clone().unwrap();
        tokio::spawn(async move {
            relay_udp_sender
                .data_sender_loop(ip, udp_port, id, data_sender)
                .await;
        });

        Ok(true)
    }
}

impl Drop for DataPublisher {
    fn drop(&mut self) {
        tracing::debug!("DataPublisher {} is dropped", self.id);
    }
}

impl Channel for DataPublisher {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn data_sender(&self) -> broadcast::Sender<DataChannelMessage> {
        self.data_sender.clone()
    }

    fn close(&self) {
        tracing::debug!("DataPublisher {} is closed", self.id);
        let _ = self.data_channel.close();
    }
}

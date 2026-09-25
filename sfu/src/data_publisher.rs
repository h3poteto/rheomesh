use std::sync::Arc;

use derivative::Derivative;
use enclose::enc;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelMessage};

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
    pub(crate) data_sender: broadcast::Sender<RTCDataChannelMessage>,
    #[derivative(Debug = "ignore")]
    data_channel: Arc<dyn DataChannel>,
    relay_sender: Arc<RelaySender>,
    relay_udp_sender: Option<Arc<RelayUDPSender>>,
}

impl DataPublisher {
    pub(crate) async fn new(
        data_channel: Arc<dyn DataChannel>,
        router_sender: mpsc::UnboundedSender<RouterEvent>,
        relay_sender: Arc<RelaySender>,
    ) -> Result<Self, Error> {
        let channel_id = data_channel.id();
        let label = data_channel.label().await?;

        let id = Uuid::new_v4().to_string();
        let (data_sender, _data_receiver) = broadcast::channel(1024);

        tokio::spawn(enc!((id, data_channel, data_sender) async move {
            Self::poll_loop(id, data_channel, data_sender, router_sender).await;
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

        Ok(publisher)
    }

    pub(crate) async fn poll_loop(
        id: String,
        data_channel: Arc<dyn DataChannel>,
        data_sender: broadcast::Sender<RTCDataChannelMessage>,
        router_sender: mpsc::UnboundedSender<RouterEvent>,
    ) {
        while let Some(event) = data_channel.poll().await {
            match event {
                DataChannelEvent::OnMessage(msg) => {
                    tracing::debug!("message: {:#?}", msg.data);
                    let _ = data_sender.send(msg);
                }
                DataChannelEvent::OnError => {
                    tracing::debug!("Error on DataChannel: {}", id);
                }
                DataChannelEvent::OnClose => {
                    break;
                }
                _ => {}
            }
        }

        tracing::debug!("DataChannel {} has been closed", id);
        let _ = router_sender.send(RouterEvent::DataRemoved(id));
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

    pub async fn close(&self) {
        tracing::debug!("DataPublisher {} is closed", self.id);
        let _ = self.data_channel.close().await;
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

    fn data_sender(&self) -> broadcast::Sender<RTCDataChannelMessage> {
        self.data_sender.clone()
    }
}

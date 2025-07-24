use tokio::sync::broadcast;
use webrtc::data_channel::data_channel_message::DataChannelMessage;

use crate::data_channel::Channel;

#[derive(Debug)]
pub struct RelayedDataPublisher {
    pub source_data_publisher_id: String,
    pub(crate) data_sender: broadcast::Sender<DataChannelMessage>,
}

impl RelayedDataPublisher {
    pub(crate) fn new(source_data_publisher_id: String) -> Self {
        let (data_sender, _data_receiver) = broadcast::channel(1024);

        tracing::debug!(
            "RelayedDataPublisher for {} created",
            source_data_publisher_id
        );
        let publisher = Self {
            source_data_publisher_id,
            data_sender,
        };

        publisher
    }
}

impl Drop for RelayedDataPublisher {
    fn drop(&mut self) {
        tracing::debug!(
            "RelayedDataPublisher for {} is dropped",
            self.source_data_publisher_id
        );
    }
}

impl Channel for RelayedDataPublisher {
    fn id(&self) -> String {
        self.source_data_publisher_id.clone()
    }

    fn data_sender(&self) -> broadcast::Sender<DataChannelMessage> {
        self.data_sender.clone()
    }
}

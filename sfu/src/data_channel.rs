use tokio::sync::broadcast;
use webrtc::data_channel::RTCDataChannelMessage;

/// Chanel represent a data channel that can be subscribed.
pub trait Channel {
    fn id(&self) -> String;
    fn data_sender(&self) -> broadcast::Sender<RTCDataChannelMessage>;
}

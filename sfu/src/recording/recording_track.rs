use derivative::Derivative;
use tokio::{net::UdpSocket, sync::broadcast};
use uuid::Uuid;
use webrtc::rtp;
use webrtc_util::Marshal;

use crate::{
    error::{Error, RecordingErrorKind},
    rtp::layer::Layer,
    utils::ports::find_unused_port,
};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct RecordingTrack {
    id: String,
    closed_sender: broadcast::Sender<bool>,
    socket: UdpSocket,
}

impl RecordingTrack {
    pub(crate) async fn new() -> Result<Self, Error> {
        let id = Uuid::new_v4().to_string();
        let (closed_sender, _closed_receiver) = broadcast::channel(1);
        let port = find_unused_port().ok_or_else(|| {
            Error::new_recording(
                "No free ports available for recording track".to_string(),
                RecordingErrorKind::PortNotFoundError,
            )
        })?;
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", port)).await?;

        tracing::debug!("Creating RecordingTrack with id: {}", id);

        Ok(Self {
            id,
            closed_sender,
            socket,
        })
    }

    pub(crate) async fn rtp_sender_loop(
        &self,
        ip: String,
        port: u16,
        track_id: String,
        rtp_sender: broadcast::Sender<(rtp::packet::Packet, Layer)>,
    ) {
        let mut rtp_receiver = rtp_sender.subscribe();
        drop(rtp_sender);
        let mut closed_receiver = self.closed_sender.subscribe();

        tracing::debug!(
            "RecordingTrack sender track_id={} RTP loop has started",
            track_id
        );

        let addr = format!("{}:{}", ip, port);

        loop {
            tokio::select! {
                _ = closed_receiver.recv() => {
                break;
                }
                res = rtp_receiver.recv() => {
                    match res {
                        Ok((packet, _layer)) => {
                            tracing::trace!("Sending recording packet: {}", track_id);
                            match packet.marshal() {
                                Ok(send_buf) => {
                                    if let Err(err) = self.socket.send_to(&send_buf, addr.clone()).await {
                                        tracing::error!("Failed to send packet with UDP: {}", err);
                                    }
                                }
                                Err(err) => {
                                    tracing::error!("Failed to marshal data: {}", err);
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            break;
                        }
                        Err(err) => {
                            tracing::error!("RecordingTrack id={} failed to read rtp: {}", track_id, err);
                        }
                    }
                }
            }
        }

        tracing::debug!(
            "RecordingTrack sender track_id={} RTP loop has finished",
            track_id
        );
    }

    pub fn close(&self) {
        let _ = self.closed_sender.send(true);
    }
}

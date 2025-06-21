use std::sync::{
    atomic::{AtomicU16, AtomicU32, Ordering},
    Arc,
};

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
        init_sequence: Arc<AtomicU16>,
        init_timestamp: Arc<AtomicU32>,
    ) {
        let mut rtp_receiver = rtp_sender.subscribe();
        drop(rtp_sender);
        let mut closed_receiver = self.closed_sender.subscribe();

        tracing::debug!(
            "RecordingTrack sender track_id={} RTP loop has started",
            track_id
        );

        let addr = format!("{}:{}", ip, port);

        let mut current_timestamp = init_timestamp.load(Ordering::Relaxed);
        let mut last_sequence_number: u16 = init_sequence.load(Ordering::Relaxed);

        loop {
            tokio::select! {
                _ = closed_receiver.recv() => {
                break;
                }
                res = rtp_receiver.recv() => {
                    match res {
                        Ok((mut packet, _layer)) => {
                            tracing::trace!("Sending recording packet: {}", track_id);

                            current_timestamp += packet.header.timestamp;
                            packet.header.timestamp = current_timestamp;
                            last_sequence_number = last_sequence_number.wrapping_add(1);
                            packet.header.sequence_number = last_sequence_number;

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

        init_sequence.store(last_sequence_number, Ordering::Relaxed);
        init_timestamp.store(current_timestamp, Ordering::Relaxed);

        tracing::debug!(
            "RecordingTrack sender track_id={} RTP loop has finished",
            track_id
        );
    }

    pub fn close(&self) {
        let _ = self.closed_sender.send(true);
    }
}

impl Drop for RecordingTrack {
    fn drop(&mut self) {
        tracing::debug!("RecordingTrack with id={} is dropped", self.id);
        self.close();
    }
}

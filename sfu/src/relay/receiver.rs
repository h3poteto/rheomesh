use std::{collections::HashMap, sync::Arc};

use bytes::Bytes;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{broadcast, Mutex},
};

use crate::{
    error::Error,
    relay::{
        data::{PacketData, TrackData},
        relayed_publisher::RelayedPublisher,
    },
    track::Track,
    worker::Worker,
};

#[derive(Debug)]
pub(crate) struct RelayServer {
    udp_socket: UdpSocket,
    tcp_listener: TcpListener,
    worker: Arc<Mutex<Worker>>,
    stop_sender: broadcast::Sender<bool>,
    publishers: Arc<Mutex<HashMap<String, Arc<Mutex<RelayedPublisher>>>>>,
}

impl RelayServer {
    pub(crate) async fn new(
        udp_port: u16,
        tcp_port: u16,
        worker: Arc<Mutex<Worker>>,
        stop_sender: broadcast::Sender<bool>,
    ) -> Result<Self, Error> {
        let udp_socket = UdpSocket::bind(format!("0.0.0.0:{}", udp_port)).await?;
        let tcp_listener = TcpListener::bind(format!("0.0.0.0:{}", tcp_port)).await?;

        Ok(Self {
            udp_socket,
            tcp_listener,
            worker,
            stop_sender,
            publishers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub(crate) async fn run_tcp(&self) -> Result<bool, Error> {
        let mut stop_receiver = self.stop_sender.subscribe();

        loop {
            tokio::select! {
                _ = stop_receiver.recv() => {
                    return Ok(false)
                }
                res = self.tcp_listener.accept() => {
                    let (stream, _) = res?;
                    if let Err(err) = self.process_tcp_stream(stream).await {
                        tracing::error!("Process tcp error: {}", err);
                    }
                }
            }
        }
    }

    async fn process_tcp_stream(&self, mut stream: TcpStream) -> Result<bool, Error> {
        let mut buffer = vec![0; 1024];

        while let Ok(n) = stream.read(&mut buffer).await {
            if n == 0 {
                break;
            }

            match serde_json::from_slice::<TrackData>(&buffer[..n]) {
                Ok(data) => {
                    tracing::debug!("tcp stream received: {:#?}", data);
                    if data.closed {
                        let locked = self.worker.lock().await;
                        match locked.routers.get(&data.router_id) {
                            Some(router) => {
                                let mut router = router.lock().await;
                                router.remove_relayd_publisher(&data.track_id).await;
                            }
                            None => {
                                stream.write_all(b"not_found").await?;
                            }
                        }
                        {
                            let mut publishers = self.publishers.lock().await;
                            if let Some(publisher) = publishers.get(&data.track_id) {
                                let locked = publisher.lock().await;
                                locked.close();
                            }
                            publishers.remove(&data.track_id);
                        }
                    } else {
                        let locked = self.worker.lock().await;
                        match locked.routers.get(&data.router_id) {
                            Some(router) => {
                                tracing::debug!("router id={} is found", data.router_id);

                                let mut publishers = self.publishers.lock().await;
                                if let Some(publisher) = publishers.get(&data.track_id) {
                                    let publisher = publisher.lock().await;
                                    publisher.create_relayed_track(
                                        data.track_id.clone(),
                                        data.ssrc,
                                        data.rid,
                                        data.mime_type,
                                        data.codec_capability.into(),
                                        data.stream_id,
                                    );
                                    stream.write_all(b"ok").await?;
                                } else {
                                    let publisher = RelayedPublisher::new(data.track_id.clone());
                                    {
                                        let publisher = publisher.lock().await;
                                        publisher.create_relayed_track(
                                            data.track_id.clone(),
                                            data.ssrc,
                                            data.rid,
                                            data.mime_type,
                                            data.codec_capability.into(),
                                            data.stream_id,
                                        );
                                    }
                                    publishers.insert(data.track_id.clone(), publisher.clone());

                                    {
                                        let mut router = router.lock().await;
                                        router
                                            .add_relayed_publisher(data.track_id.clone(), publisher)
                                            .await;
                                    }
                                }

                                stream.write_all(b"ok").await?;
                            }
                            None => {
                                tracing::warn!("router id={} is not found", data.router_id);
                                stream.write_all(b"not_found").await?;
                            }
                        }
                    }
                }
                Err(err) => {
                    tracing::error!("failed to parse tcp stream: {}", err);
                    stream.write_all(b"error").await?;
                }
            }
        }

        Ok(true)
    }

    pub(crate) async fn run_udp(&self) -> Result<bool, Error> {
        loop {
            let mut buf = [0u8; 1500];
            let (len, _addr) = self.udp_socket.recv_from(&mut buf).await?;

            let bytes = Bytes::copy_from_slice(&buf[..len]);
            let packet_data = PacketData::unmarshal(&bytes, len)?;

            tracing::trace!("packet received with udp: {:#?}", packet_data);

            let publishers = self.publishers.lock().await;
            if let Some(publisher) = publishers.get(&packet_data.track_id) {
                let publisher = publisher.lock().await;
                if let Some(track) = publisher.local_tracks.get(&packet_data.ssrc) {
                    let rtp_packet_sender = track.rtp_packet_sender();
                    if rtp_packet_sender.receiver_count() > 0 {
                        if let Err(err) =
                            rtp_packet_sender.send((packet_data.packet, packet_data.layer))
                        {
                            tracing::error!(
                                "RelayedTrack id={} ssrc={} failed to send rtp: {}",
                                packet_data.track_id,
                                packet_data.ssrc,
                                err
                            );
                        }
                    }
                }
            }
        }
    }
}

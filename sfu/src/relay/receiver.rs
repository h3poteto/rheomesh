use std::{collections::HashMap, sync::Arc};

use bincode::{Decode, Encode};
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

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
struct RouterId(String);

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
struct PublisherId(String);

#[derive(Debug)]
pub(crate) struct RelayServer {
    udp_socket: UdpSocket,
    tcp_listener: TcpListener,
    worker: Arc<Mutex<Worker>>,
    stop_sender: broadcast::Sender<bool>,
    publishers: Arc<Mutex<HashMap<PublisherId, HashMap<RouterId, Arc<Mutex<RelayedPublisher>>>>>>,
    udp_port: u16,
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
            udp_port,
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
                    let res = self.handle_tcp_message(data).await;
                    let byte = bincode::encode_to_vec(res, bincode::config::standard()).unwrap();
                    stream.write_all(&byte).await?;
                }
                Err(err) => {
                    tracing::error!("failed to parse tcp stream: {}", err);
                    stream.write_all(b"error").await?;
                }
            }
        }

        Ok(true)
    }

    async fn handle_tcp_message(&self, data: TrackData) -> TCPResponse {
        tracing::debug!("tcp stream received: {:#?}", data);
        let router_id = RouterId(data.router_id.clone());
        let publisher_id = PublisherId(data.track_id.clone());
        if data.closed {
            let locked = self.worker.lock().await;
            match locked.routers.get(&data.router_id) {
                Some(router) => {
                    let mut router = router.lock().await;
                    router.remove_relayd_publisher(&data.track_id).await;
                }
                None => {
                    return TCPResponse {
                        status: "error".to_string(),
                        message: "router not found".to_string(),
                        port: None,
                    }
                }
            }

            let mut publishers = self.publishers.lock().await;

            if let Some(router_publisher) = publishers.get(&publisher_id) {
                if let Some(publisher) = router_publisher.get(&router_id) {
                    let locked = publisher.lock().await;
                    locked.close();
                }
            }
            publishers.remove(&publisher_id);

            return TCPResponse {
                status: "ok".to_string(),
                message: "publisher removed".to_string(),
                port: None,
            };
        } else {
            let locked = self.worker.lock().await;
            match locked.routers.get(&data.router_id) {
                Some(router) => {
                    tracing::debug!("router id={} is found", data.router_id);

                    let mut publishers = self.publishers.lock().await;
                    if let Some(publisher) = publishers
                        .get(&publisher_id)
                        .and_then(|p| p.get(&router_id))
                    {
                        let mut publisher = publisher.lock().await;
                        publisher.publisher_type = data.publisher_type;
                        publisher.create_relayed_track(
                            data.track_id.clone(),
                            data.ssrc,
                            data.rid,
                            data.mime_type,
                            data.codec_capability.into(),
                            data.stream_id,
                        );
                    } else {
                        let publisher =
                            RelayedPublisher::new(data.track_id.clone(), data.publisher_type);
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

                        {
                            let mut router = router.lock().await;
                            router
                                .add_relayed_publisher(data.track_id.clone(), publisher.clone())
                                .await;
                        }

                        if let Some(router_publisher) = publishers.get_mut(&publisher_id) {
                            router_publisher.insert(router_id.clone(), publisher.clone());
                        } else {
                            publishers.insert(
                                publisher_id.clone(),
                                HashMap::from([(router_id.clone(), publisher.clone())]),
                            );
                        }
                    }

                    return TCPResponse {
                        status: "ok".to_string(),
                        message: "publisher added".to_string(),
                        port: Some(self.udp_port),
                    };
                }
                None => {
                    tracing::warn!("router id={} is not found", data.router_id);
                    return TCPResponse {
                        status: "error".to_string(),
                        message: "router not found".to_string(),
                        port: None,
                    };
                }
            }
        }
    }

    pub(crate) async fn run_udp(&self) -> Result<bool, Error> {
        loop {
            let mut buf = [0u8; 1500];
            let (len, _addr) = self.udp_socket.recv_from(&mut buf).await?;

            let bytes = Bytes::copy_from_slice(&buf[..len]);
            let packet_data = PacketData::unmarshal(&bytes, len)?;

            tracing::trace!("packet received with udp: {:#?}", packet_data);

            let publisher_id = PublisherId(packet_data.track_id.clone());

            let publishers = self.publishers.lock().await;
            if let Some(router_publisher) = publishers.get(&publisher_id) {
                for (_router_id, publisher) in router_publisher.iter() {
                    let data = packet_data.clone();
                    let publisher = publisher.lock().await;
                    if let Some(track) = publisher.local_tracks.get(&data.ssrc) {
                        let rtp_packet_sender = track.rtp_packet_sender();
                        if rtp_packet_sender.receiver_count() > 0 {
                            if let Err(err) = rtp_packet_sender.send((data.packet, data.layer)) {
                                tracing::error!(
                                    "RelayedTrack id={} ssrc={} failed to send rtp: {}",
                                    data.track_id,
                                    data.ssrc,
                                    err
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub(crate) struct TCPResponse {
    pub(crate) status: String,
    pub(crate) message: String,
    pub(crate) port: Option<u16>,
}

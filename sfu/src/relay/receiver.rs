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
    utils::ports::find_unused_port,
    worker::Worker,
};

use super::data::UDPStarted;

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
struct RouterId(String);

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
struct PublisherId(String);

#[derive(Debug)]
pub(crate) struct RelayServer {
    tcp_listener: TcpListener,
    worker: Arc<Mutex<Worker>>,
    stop_sender: broadcast::Sender<bool>,
    publishers: Arc<
        Mutex<HashMap<RouterId, Arc<Mutex<HashMap<PublisherId, Arc<Mutex<RelayedPublisher>>>>>>>,
    >,
    udp_servers: Arc<Mutex<HashMap<RouterId, Arc<Mutex<RelayUDPServer>>>>>,
}

impl RelayServer {
    pub(crate) async fn new(
        tcp_port: u16,
        worker: Arc<Mutex<Worker>>,
        stop_sender: broadcast::Sender<bool>,
    ) -> Result<Self, Error> {
        let tcp_listener = TcpListener::bind(format!("0.0.0.0:{}", tcp_port)).await?;

        Ok(Self {
            tcp_listener,
            worker,
            stop_sender,
            publishers: Arc::new(Mutex::new(HashMap::new())),
            udp_servers: Arc::new(Mutex::new(HashMap::new())),
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
                    tracing::debug!("TCP response: {:#?}", res);
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
                        message: Some("router not found".to_string()),
                        udp_started: None,
                    }
                }
            }

            let mut publishers = self.publishers.lock().await;

            if let Some(router_publisher) = publishers.get(&router_id) {
                if let Some(publisher) = router_publisher.lock().await.get(&publisher_id) {
                    let locked = publisher.lock().await;
                    locked.close();
                }
            }
            if let Some(router_publisher) = publishers.get_mut(&router_id) {
                router_publisher.lock().await.remove(&publisher_id);
                if router_publisher.lock().await.is_empty() {
                    // Stop UDP receiver server if no publishers left
                    let mut servers = self.udp_servers.lock().await;
                    if let Some(udp_server) = servers.get(&router_id) {
                        udp_server.lock().await.close();
                        servers.remove(&router_id);
                    }
                    publishers.remove(&router_id);
                }
            }

            return TCPResponse {
                status: "ok".to_string(),
                message: Some("publisher removed".to_string()),
                udp_started: None,
            };
        } else {
            let locked = self.worker.lock().await;
            match locked.routers.get(&data.router_id) {
                Some(router) => {
                    tracing::debug!("router id={} is found", data.router_id);

                    let mut publishers = self.publishers.lock().await;
                    if let Some(router_publisher) = publishers.get_mut(&router_id) {
                        if let Some(publisher) = router_publisher.lock().await.get(&publisher_id) {
                            // For simulcast.
                            // If this server receives another track with the same track_id, it is a simulcast track with another resolution.
                            // It should be the same track_id, but different ssrc and rid.
                            let mut publisher = publisher.lock().await;
                            publisher.publisher_type = data.publisher_type;
                            publisher.create_relayed_track(
                                data.track_id.clone(),
                                data.ssrc,
                                data.rid,
                                data.mime_type,
                                data.codec_parameters.into(),
                                data.stream_id,
                            );
                        } else {
                            let publisher = self.create_publisher(&data).await;

                            {
                                let mut router = router.lock().await;
                                router
                                    .add_relayed_publisher(data.track_id.clone(), publisher.clone())
                                    .await;
                            }
                            router_publisher
                                .lock()
                                .await
                                .insert(publisher_id.clone(), publisher.clone());
                        }
                    } else {
                        if let Some(udp_port) = find_unused_port() {
                            let publisher = self.create_publisher(&data).await;

                            {
                                let mut router = router.lock().await;
                                router
                                    .add_relayed_publisher(data.track_id.clone(), publisher.clone())
                                    .await;
                            }

                            publishers.insert(
                                router_id.clone(),
                                Arc::new(Mutex::new(HashMap::from([(
                                    publisher_id.clone(),
                                    publisher.clone(),
                                )]))),
                            );

                            let p = publishers.get(&router_id).cloned().unwrap_or_default();
                            match RelayUDPServer::new(udp_port, p).await {
                                Ok(udp) => {
                                    let mut udp_servers = self.udp_servers.lock().await;
                                    udp_servers
                                        .insert(router_id.clone(), Arc::new(Mutex::new(udp)));
                                }
                                Err(err) => {
                                    tracing::error!("Failed to create UDP server: {}", err);
                                    return TCPResponse {
                                        status: "error".to_string(),
                                        message: Some("failed to create UDP server".to_string()),
                                        udp_started: None,
                                    };
                                }
                            }
                        } else {
                            tracing::error!("No free UDP port found for router {}", data.router_id);
                            return TCPResponse {
                                status: "error".to_string(),
                                message: Some("no free UDP port found".to_string()),
                                udp_started: None,
                            };
                        }
                    }

                    let udp_server_port: u16;
                    if let Some(udp_server) = self.udp_servers.lock().await.get(&router_id) {
                        udp_server_port = udp_server.lock().await.udp_port;
                    } else {
                        tracing::error!("No UDP server found for router {}", data.router_id);
                        return TCPResponse {
                            status: "error".to_string(),
                            message: Some("no UDP server found".to_string()),
                            udp_started: None,
                        };
                    }

                    return TCPResponse {
                        status: "ok".to_string(),
                        message: None,
                        udp_started: Some(UDPStarted {
                            port: udp_server_port,
                        }),
                    };
                }
                None => {
                    tracing::warn!("router id={} is not found", data.router_id);
                    return TCPResponse {
                        status: "error".to_string(),
                        message: Some("router not found".to_string()),
                        udp_started: None,
                    };
                }
            }
        }
    }

    async fn create_publisher(&self, data: &TrackData) -> Arc<Mutex<RelayedPublisher>> {
        let rtcp_ip = data.ip.clone().unwrap();
        let rtcp_port = data.udp_port.unwrap_or(0);
        let publisher = RelayedPublisher::new(
            data.track_id.clone(),
            data.publisher_type.clone(),
            rtcp_ip,
            rtcp_port,
        );
        {
            let mut publisher = publisher.lock().await;
            publisher.create_relayed_track(
                data.track_id.clone(),
                data.ssrc,
                data.rid.clone(),
                data.mime_type.clone(),
                data.codec_parameters.clone().into(),
                data.stream_id.clone(),
            );
        }
        publisher
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub(crate) struct TCPResponse {
    pub(crate) status: String,
    pub(crate) udp_started: Option<UDPStarted>,
    pub(crate) message: Option<String>,
}

#[derive(Debug)]
pub(crate) struct RelayUDPServer {
    pub(crate) udp_port: u16,
    closed: broadcast::Sender<bool>,
}

impl RelayUDPServer {
    async fn new(
        udp_port: u16,
        publishers: Arc<Mutex<HashMap<PublisherId, Arc<Mutex<RelayedPublisher>>>>>,
    ) -> Result<Self, Error> {
        let (closed_sender, _closed_receiver) = broadcast::channel(1);

        {
            let closed_sender = closed_sender.clone();
            tokio::spawn(async move {
                if let Err(err) = Self::run_udp(udp_port, publishers, closed_sender.clone()).await {
                    tracing::error!("UDP server error: {}", err);
                }
            });
        }

        Ok(Self {
            udp_port,
            closed: closed_sender,
        })
    }

    async fn run_udp(
        udp_port: u16,
        publishers: Arc<Mutex<HashMap<PublisherId, Arc<Mutex<RelayedPublisher>>>>>,
        closed_sender: broadcast::Sender<bool>,
    ) -> Result<bool, Error> {
        let mut closed_receiver = closed_sender.subscribe();
        let udp_socket = UdpSocket::bind(format!("0.0.0.0:{}", udp_port)).await?;
        tracing::info!("UDP server started on :{}", udp_port);

        loop {
            let mut buf = [0u8; 1500];

            tokio::select! {
                _ = closed_receiver.recv() => {
                    tracing::info!("UDP server on :{} is closed", udp_port);
                    break;
                }
                res = udp_socket.recv_from(&mut buf) => {
                    let (len, _addr) = res?;
                    let bytes = Bytes::copy_from_slice(&buf[..len]);
                    let packet_data = PacketData::unmarshal(&bytes, len)?;

                    tracing::trace!("packet received with udp: {:#?}", packet_data);

                    let publisher_id = PublisherId(packet_data.track_id.clone());

                    let publishers = publishers.lock().await;
                    if let Some(publisher) = publishers.get(&publisher_id) {
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

        Ok(true)
    }

    pub(crate) fn close(&self) {
        self.closed.send(true).unwrap();
    }
}

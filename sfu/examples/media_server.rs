use std::collections::HashMap;
use std::env;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use actix::{Actor, Addr, Message, StreamHandler};
use actix::{AsyncContext, Handler};
use actix_web::web::{Data, Query};
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use actix_web_actors::ws;
use rheomesh::config::{CodecConfig, MediaConfig};
use rheomesh::publisher::Publisher;
use rheomesh::recording::recording_track::RecordingTrack;
use rheomesh::recording::recording_transport::RecordingTransport;
use rheomesh::subscriber::Subscriber;
use rheomesh::transport::Transport;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing_actix_web::TracingLogger;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use webrtc::api::media_engine;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters};
use webrtc::rtp_transceiver::RTCPFeedback;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let worker = rheomesh::worker::Worker::new(rheomesh::config::WorkerConfig::default())
        .await
        .unwrap();
    let room_owner = RoomOwner::new(worker);
    let room_data = Data::new(Mutex::new(room_owner));

    HttpServer::new(move || {
        App::new()
            .wrap(TracingLogger::default())
            .service(index)
            .app_data(room_data.clone())
            .route("/socket", web::get().to(socket))
    })
    .bind("0.0.0.0:4000")?
    .run()
    .await
}

#[actix_web::get("/")]
async fn index() -> impl Responder {
    HttpResponse::Ok().body("healthy")
}

async fn socket(
    req: HttpRequest,
    room_owner: Data<Mutex<RoomOwner>>,
    stream: web::Payload,
) -> impl Responder {
    let query = req.query_string();

    let parameters =
        Query::<HashMap<String, String>>::from_query(query).expect("Failed to parse query");
    let room_id = parameters.get("room").expect("room is required");
    let find = room_owner
        .as_ref()
        .lock()
        .await
        .find_by_id(room_id.to_string());

    let mut config = MediaConfig::default();
    config.codec = CodecConfig {
        audio: audio_codecs(),
        video: video_codecs(),
    };

    match find {
        Some(room) => {
            tracing::info!("Room found, so joining it: {}", room_id);
            let server = WebSocket::new(room, room_owner.clone()).await;
            ws::start(server, &req, stream)
        }
        None => {
            let owner = room_owner.clone();
            let mut owner = owner.lock().await;
            let room = owner.create_new_room(room_id.to_string(), config).await;
            let server = WebSocket::new(room, room_owner.clone()).await;
            ws::start(server, &req, stream)
        }
    }
}

struct WebSocket {
    owner: Data<Mutex<RoomOwner>>,
    room: Arc<Room>,
    publish_transport: Arc<rheomesh::publish_transport::PublishTransport>,
    subscribe_transport: Arc<rheomesh::subscribe_transport::SubscribeTransport>,
    publishers: Arc<Mutex<HashMap<String, Arc<Mutex<Publisher>>>>>,
    subscribers: Arc<Mutex<HashMap<String, Arc<Mutex<Subscriber>>>>>,
    recording_transport: Arc<Option<RecordingTransport>>,
    recordings: Arc<Mutex<HashMap<String, Arc<RecordingTrack>>>>,
}

impl WebSocket {
    // This function is called when a new user connect to this server.
    pub async fn new(room: Arc<Room>, owner: Data<Mutex<RoomOwner>>) -> Self {
        tracing::info!("Starting WebSocket");
        let r = room.router.clone();
        let router = r.lock().await;

        let mut config = rheomesh::config::WebRTCTransportConfig::default();
        // Public IP address of your server.
        let ip = env::var("PUBLIC_IP").expect("PUBLIC_IP is required");
        let ipv4 = ip
            .parse::<Ipv4Addr>()
            .expect("failed to parse public IP address");
        config.announced_ips = vec![IpAddr::V4(ipv4)];
        config.configuration.ice_servers = vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }];
        // Port range of your server.
        config.port_range = Some(rheomesh::config::PortRange {
            min: 12000,
            max: 15000,
        });

        let publish_transport = router.create_publish_transport(config.clone()).await;
        let subscribe_transport = router.create_subscribe_transport(config).await;

        let recording_host = env::var("RECORDING_HOST").unwrap_or("127.0.0.1".to_string());
        let recording_port = env::var("RECORDING_PORT")
            .unwrap_or("15000".to_string())
            .parse::<u16>()
            .expect("RECORDING_PORT must be a valid port number");
        let recording_transport = router
            .create_recording_transport(recording_host, recording_port)
            .await
            .ok();
        Self {
            owner,
            room,
            publish_transport: Arc::new(publish_transport),
            subscribe_transport: Arc::new(subscribe_transport),
            publishers: Arc::new(Mutex::new(HashMap::new())),
            subscribers: Arc::new(Mutex::new(HashMap::new())),
            recording_transport: Arc::new(recording_transport),
            recordings: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Actor for WebSocket {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        tracing::info!("New WebSocket connection is started");
        let address = ctx.address();
        self.room.add_user(address.clone());
    }

    fn stopped(&mut self, ctx: &mut Self::Context) {
        tracing::info!("The WebSocket connection is stopped");
        let address = ctx.address();
        let subscribe_transport = self.subscribe_transport.clone();
        let publish_transport = self.publish_transport.clone();
        actix::spawn(async move {
            subscribe_transport
                .close()
                .await
                .expect("failed to close subscribe_transport");
            publish_transport
                .close()
                .await
                .expect("failed to close publish_transport");
        });
        let users = self.room.remove_user(address);
        if users == 0 {
            let owner = self.owner.clone();
            let router = self.room.router.clone();
            let room_id = self.room.id.clone();
            actix::spawn(async move {
                let mut owner = owner.lock().await;
                owner.remove_room(room_id);
                let router = router.lock().await;
                router.close();
            });
        }
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WebSocket {
    fn handle(&mut self, item: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match item {
            Ok(ws::Message::Ping(msg)) => ctx.pong(&msg),
            Ok(ws::Message::Pong(_)) => tracing::info!("pong received"),
            Ok(ws::Message::Text(text)) => match serde_json::from_str::<ReceivedMessage>(&text) {
                Ok(message) => {
                    ctx.address().do_send(message);
                }
                Err(error) => {
                    tracing::error!("failed to parse client message: {}\n{}", error, text);
                }
            },
            Ok(ws::Message::Binary(bin)) => ctx.binary(bin),
            Ok(ws::Message::Close(reason)) => ctx.close(reason),
            _ => (),
        }
    }
}

impl Handler<ReceivedMessage> for WebSocket {
    type Result = ();

    fn handle(&mut self, msg: ReceivedMessage, ctx: &mut Self::Context) -> Self::Result {
        let address = ctx.address();
        tracing::debug!("received message: {:?}", msg);

        match msg {
            ReceivedMessage::Ping => {
                address.do_send(SendingMessage::Pong);
            }
            ReceivedMessage::PublisherInit => {
                let publish_transport = self.publish_transport.clone();
                tokio::spawn(async move {
                    let addr = address.clone();
                    publish_transport
                        .on_ice_candidate(Box::new(move |candidate| {
                            let init = candidate.to_json().expect("failed to parse candidate");
                            addr.do_send(SendingMessage::PublisherIce { candidate: init });
                        }))
                        .await;
                });
            }
            ReceivedMessage::SubscriberInit => {
                let subscribe_transport = self.subscribe_transport.clone();
                let room = self.room.clone();
                tokio::spawn(async move {
                    let addr = address.clone();
                    let addr2 = address.clone();
                    subscribe_transport
                        .on_ice_candidate(Box::new(move |candidate| {
                            let init = candidate.to_json().expect("failed to parse candidate");
                            addr.do_send(SendingMessage::SubscriberIce { candidate: init });
                        }))
                        .await;
                    subscribe_transport
                        .on_negotiation_needed(Box::new(move |offer| {
                            addr2.do_send(SendingMessage::Offer { sdp: offer });
                        }))
                        .await;

                    let router = room.router.lock().await;
                    let ids = router.publisher_ids();
                    tracing::info!("router publisher ids {:#?}", ids);
                    address.do_send(SendingMessage::Published { publisher_ids: ids });
                });
            }

            ReceivedMessage::RequestPublish => address.do_send(SendingMessage::StartAsPublisher),
            ReceivedMessage::PublisherIce { candidate } => {
                let publish_transport = self.publish_transport.clone();
                actix::spawn(async move {
                    let _ = publish_transport
                        .add_ice_candidate(candidate)
                        .await
                        .expect("failed to add ICE candidate");
                });
            }
            ReceivedMessage::SubscriberIce { candidate } => {
                let subscribe_transport = self.subscribe_transport.clone();
                actix::spawn(async move {
                    let _ = subscribe_transport
                        .add_ice_candidate(candidate)
                        .await
                        .expect("failed to add ICE candidate");
                });
            }
            ReceivedMessage::Offer { sdp } => {
                let publish_transport = self.publish_transport.clone();
                actix::spawn(async move {
                    let answer = publish_transport
                        .get_answer(sdp)
                        .await
                        .expect("failed to connect publish_transport");

                    address.do_send(SendingMessage::Answer { sdp: answer });
                });
            }
            ReceivedMessage::Subscribe { publisher_id } => {
                let subscribe_transport = self.subscribe_transport.clone();
                let subscribers = self.subscribers.clone();
                actix::spawn(async move {
                    let (subscriber, offer) = subscribe_transport
                        .subscribe(publisher_id)
                        .await
                        .expect("failed to connect subscribe_transport");

                    #[allow(unused)]
                    let mut id = "".to_owned();
                    {
                        let guard = subscriber.lock().await;
                        id = guard.id.clone();
                    }
                    let mut s = subscribers.lock().await;
                    s.insert(id.clone(), subscriber);
                    address.do_send(SendingMessage::Offer { sdp: offer });
                    address.do_send(SendingMessage::Subscribed { subscriber_id: id })
                });
            }
            ReceivedMessage::Answer { sdp } => {
                let subscribe_transport = self.subscribe_transport.clone();
                actix::spawn(async move {
                    let _ = subscribe_transport
                        .set_answer(sdp)
                        .await
                        .expect("failed to set answer");
                });
            }
            ReceivedMessage::Publish { publisher_id } => {
                let room = self.room.clone();
                let publish_transport = self.publish_transport.clone();
                let publishers = self.publishers.clone();
                actix::spawn(async move {
                    match publish_transport.publish(publisher_id).await {
                        Ok(publisher) => {
                            #[allow(unused)]
                            let mut track_id = "".to_owned();
                            {
                                let publisher = publisher.lock().await;
                                track_id = publisher.track_id.clone();
                            }
                            tracing::debug!("published a track: {}", track_id);
                            // address.do_send(SendingMessage::Published {
                            //     track_id: id.clone(),
                            // });
                            let mut p = publishers.lock().await;
                            p.insert(track_id.clone(), publisher.clone());
                            room.get_peers(&address).iter().for_each(|peer| {
                                peer.do_send(SendingMessage::Published {
                                    publisher_ids: vec![track_id.clone()],
                                });
                            });
                        }
                        Err(err) => {
                            tracing::error!("{}", err);
                        }
                    }
                });
            }
            ReceivedMessage::StopPublish { publisher_id } => {
                let publishers = self.publishers.clone();
                actix::spawn(async move {
                    let mut p = publishers.lock().await;
                    if let Some(publisher) = p.remove(&publisher_id) {
                        let publisher = publisher.lock().await;
                        publisher.close().await;
                    }
                });
            }
            ReceivedMessage::StopSubscribe { subscriber_id } => {
                let subscribers = self.subscribers.clone();
                actix::spawn(async move {
                    let mut s = subscribers.lock().await;
                    if let Some(subscriber) = s.remove(&subscriber_id) {
                        let subscriber = subscriber.lock().await;
                        subscriber.close().await;
                    }
                });
            }
            ReceivedMessage::SetPreferredLayer {
                subscriber_id,
                sid,
                tid,
            } => {
                let subscribers = self.subscribers.clone();
                actix::spawn(async move {
                    let s = subscribers.lock().await;
                    if let Some(subscriber) = s.get(&subscriber_id) {
                        let mut subscriber = subscriber.lock().await;
                        if let Err(err) = subscriber.set_preferred_layer(sid, tid).await {
                            tracing::error!("Failed to set preferred layer: {}", err);
                        }
                    }
                });
            }
            ReceivedMessage::RestartICE {} => {
                let subscribe_transport = self.subscribe_transport.clone();
                actix::spawn(async move {
                    match subscribe_transport.restart_ice().await {
                        Ok(offer) => {
                            address.do_send(SendingMessage::Offer { sdp: offer });
                        }
                        Err(err) => {
                            tracing::error!("Failed to restart ICE: {}", err);
                        }
                    }
                });
            }
            ReceivedMessage::Record { publisher_id } => {
                let recording_transport = self.recording_transport.clone();
                actix::spawn(async move {
                    match recording_transport.as_ref() {
                        Some(transport) => match transport.generate_sdp(publisher_id.clone()).await
                        {
                            Ok(sdp) => {
                                tracing::info!("Generated SDP for recording: \n{}", sdp);
                                address.do_send(SendingMessage::RecordingSDP { sdp });
                            }
                            Err(err) => {
                                tracing::error!("Failed to generate SDP for recording: {}", err);
                                address.do_send(SendingMessage::RecordingError {
                                    error: err.to_string(),
                                });
                            }
                        },
                        None => {
                            tracing::error!("Failed to create recording transport");
                            address.do_send(SendingMessage::RecordingError {
                                error: "Recording transport is not available".to_string(),
                            });
                        }
                    }
                });
            }
            ReceivedMessage::StartRecording { publisher_id } => {
                let recording_transport = self.recording_transport.clone();
                let recordings = self.recordings.clone();
                actix::spawn(async move {
                    if let Some(transport) = recording_transport.as_ref() {
                        match transport.start_recording(publisher_id.clone()).await {
                            Ok(track) => {
                                tracing::info!("Recording started successfully");
                                recordings.lock().await.insert(publisher_id, track);
                            }
                            Err(err) => tracing::error!("Failed to start recording: {}", err),
                        }
                    } else {
                        tracing::error!("Recording transport is not available");
                    }
                });
            }
            ReceivedMessage::StopRecording { publisher_id } => {
                let recordings = self.recordings.clone();
                actix::spawn(async move {
                    let mut r = recordings.lock().await;
                    if let Some(track) = r.remove(&publisher_id) {
                        track.close();
                        tracing::info!("Recording stopped for publisher: {}", publisher_id);
                    } else {
                        tracing::warn!("No recording found for publisher: {}", publisher_id);
                    }
                });
            }
        }
    }
}

impl Handler<SendingMessage> for WebSocket {
    type Result = ();

    fn handle(&mut self, msg: SendingMessage, ctx: &mut Self::Context) -> Self::Result {
        tracing::debug!("sending message: {:?}", msg);
        ctx.text(serde_json::to_string(&msg).expect("failed to parse SendingMessage"));
    }
}

impl Handler<InternalMessage> for WebSocket {
    type Result = ();

    fn handle(&mut self, _msg: InternalMessage, _ctx: &mut Self::Context) -> Self::Result {}
}

#[derive(Deserialize, Message, Debug)]
#[serde(tag = "action")]
#[rtype(result = "()")]
enum ReceivedMessage {
    #[serde(rename_all = "camelCase")]
    Ping,
    #[serde(rename_all = "camelCase")]
    PublisherInit,
    #[serde(rename_all = "camelCase")]
    SubscriberInit,
    #[serde(rename_all = "camelCase")]
    RequestPublish,
    // Seems like client-side (JS) RTCIceCandidate struct is equal RTCIceCandidateInit.
    #[serde(rename_all = "camelCase")]
    PublisherIce { candidate: RTCIceCandidateInit },
    #[serde(rename_all = "camelCase")]
    SubscriberIce { candidate: RTCIceCandidateInit },
    #[serde(rename_all = "camelCase")]
    Offer { sdp: RTCSessionDescription },
    #[serde(rename_all = "camelCase")]
    Subscribe { publisher_id: String },
    #[serde(rename_all = "camelCase")]
    Answer { sdp: RTCSessionDescription },
    #[serde(rename_all = "camelCase")]
    Publish { publisher_id: String },
    #[serde(rename_all = "camelCase")]
    StopPublish { publisher_id: String },
    #[serde(rename_all = "camelCase")]
    StopSubscribe { subscriber_id: String },
    #[serde(rename_all = "camelCase")]
    SetPreferredLayer {
        subscriber_id: String,
        sid: u8,
        tid: Option<u8>,
    },
    #[serde(rename_all = "camelCase")]
    RestartICE,
    #[serde(rename_all = "camelCase")]
    Record { publisher_id: String },
    #[serde(rename_all = "camelCase")]
    StartRecording { publisher_id: String },
    #[serde(rename_all = "camelCase")]
    StopRecording { publisher_id: String },
}

#[derive(Serialize, Message, Debug)]
#[serde(tag = "action")]
#[rtype(result = "()")]
enum SendingMessage {
    #[serde(rename_all = "camelCase")]
    Pong,
    #[serde(rename_all = "camelCase")]
    StartAsPublisher,
    #[serde(rename_all = "camelCase")]
    Answer { sdp: RTCSessionDescription },
    #[serde(rename_all = "camelCase")]
    Offer { sdp: RTCSessionDescription },
    #[serde(rename_all = "camelCase")]
    PublisherIce { candidate: RTCIceCandidateInit },
    #[serde(rename_all = "camelCase")]
    SubscriberIce { candidate: RTCIceCandidateInit },
    #[serde(rename_all = "camelCase")]
    Published { publisher_ids: Vec<String> },
    #[serde(rename_all = "camelCase")]
    Subscribed { subscriber_id: String },
    #[serde(rename_all = "camelCase")]
    RecordingSDP { sdp: String },
    #[serde(rename_all = "camelCase")]
    RecordingError { error: String },
}

#[derive(Message, Debug)]
#[rtype(result = "()")]
enum InternalMessage {}

struct RoomOwner {
    rooms: HashMap<String, Arc<Room>>,
    worker: Arc<Mutex<rheomesh::worker::Worker>>,
}

impl RoomOwner {
    pub fn new(worker: Arc<Mutex<rheomesh::worker::Worker>>) -> Self {
        RoomOwner {
            rooms: HashMap::<String, Arc<Room>>::new(),
            worker,
        }
    }

    fn find_by_id(&self, id: String) -> Option<Arc<Room>> {
        self.rooms.get(&id).cloned()
    }

    async fn create_new_room(
        &mut self,
        id: String,
        config: rheomesh::config::MediaConfig,
    ) -> Arc<Room> {
        let mut worker = self.worker.lock().await;
        let router = worker.new_router(config);
        let room = Room::new(id.clone(), router);
        let a = Arc::new(room);
        self.rooms.insert(id.clone(), a.clone());
        a
    }

    fn remove_room(&mut self, room_id: String) {
        self.rooms.remove(&room_id);
    }
}

struct Room {
    id: String,
    pub router: Arc<Mutex<rheomesh::router::Router>>,
    users: std::sync::Mutex<Vec<Addr<WebSocket>>>,
}

impl Room {
    pub fn new(id: String, router: Arc<Mutex<rheomesh::router::Router>>) -> Self {
        Self {
            id,
            router,
            users: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn add_user(&self, user: Addr<WebSocket>) {
        let mut users = self.users.lock().unwrap();
        users.push(user);
    }

    pub fn remove_user(&self, user: Addr<WebSocket>) -> usize {
        let mut users = self.users.lock().unwrap();
        users.retain(|u| u != &user);
        users.len()
    }

    pub fn get_peers(&self, user: &Addr<WebSocket>) -> Vec<Addr<WebSocket>> {
        let users = self.users.lock().unwrap();
        users.iter().filter(|u| u != &user).cloned().collect()
    }
}

fn audio_codecs() -> Vec<RTCRtpCodecParameters> {
    return vec![
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: 111,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_G722.to_owned(),
                clock_rate: 8000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: 9,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_PCMU.to_owned(),
                clock_rate: 8000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: 0,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_PCMA.to_owned(),
                clock_rate: 8000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: 8,
            ..Default::default()
        },
    ];
}

fn video_codecs() -> Vec<RTCRtpCodecParameters> {
    let video_rtcp_feedback = vec![
        RTCPFeedback {
            typ: "goog-remb".to_owned(),
            parameter: "".to_owned(),
        },
        RTCPFeedback {
            typ: "ccm".to_owned(),
            parameter: "fir".to_owned(),
        },
        RTCPFeedback {
            typ: "nack".to_owned(),
            parameter: "".to_owned(),
        },
        RTCPFeedback {
            typ: "nack".to_owned(),
            parameter: "pli".to_owned(),
        },
    ];
    return vec![
        // RTCRtpCodecParameters {
        //     capability: RTCRtpCodecCapability {
        //         mime_type: media_engine::MIME_TYPE_VP8.to_owned(),
        //         clock_rate: 90000,
        //         channels: 0,
        //         sdp_fmtp_line: "".to_owned(),
        //         rtcp_feedback: video_rtcp_feedback.clone(),
        //     },
        //     payload_type: 96,
        //     ..Default::default()
        // },
        // RTCRtpCodecParameters {
        //     capability: RTCRtpCodecCapability {
        //         mime_type: media_engine::MIME_TYPE_VP9.to_owned(),
        //         clock_rate: 90000,
        //         channels: 0,
        //         sdp_fmtp_line: "profile-id=0".to_owned(),
        //         rtcp_feedback: video_rtcp_feedback.clone(),
        //     },
        //     payload_type: 98,
        //     ..Default::default()
        // },
        // RTCRtpCodecParameters {
        //     capability: RTCRtpCodecCapability {
        //         mime_type: media_engine::MIME_TYPE_VP9.to_owned(),
        //         clock_rate: 90000,
        //         channels: 0,
        //         sdp_fmtp_line: "profile-id=1".to_owned(),
        //         rtcp_feedback: video_rtcp_feedback.clone(),
        //     },
        //     payload_type: 100,
        //     ..Default::default()
        // },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                        .to_owned(),
                rtcp_feedback: video_rtcp_feedback.clone(),
            },
            payload_type: 102,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42001f"
                        .to_owned(),
                rtcp_feedback: video_rtcp_feedback.clone(),
            },
            payload_type: 127,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                        .to_owned(),
                rtcp_feedback: video_rtcp_feedback.clone(),
            },
            payload_type: 125,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42e01f"
                        .to_owned(),
                rtcp_feedback: video_rtcp_feedback.clone(),
            },
            payload_type: 108,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=0;profile-level-id=42001f"
                        .to_owned(),
                rtcp_feedback: video_rtcp_feedback.clone(),
            },
            payload_type: 127,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=640032"
                        .to_owned(),
                rtcp_feedback: video_rtcp_feedback.clone(),
            },
            payload_type: 123,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_AV1.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile-id=0".to_owned(),
                rtcp_feedback: video_rtcp_feedback.clone(),
            },
            payload_type: 45,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: media_engine::MIME_TYPE_HEVC.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: video_rtcp_feedback,
            },
            payload_type: 126,
            ..Default::default()
        },
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: "video/ulpfec".to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: 116,
            ..Default::default()
        },
    ];
}

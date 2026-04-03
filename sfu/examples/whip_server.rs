use std::env;
use std::net::{IpAddr, Ipv4Addr};
use std::{collections::HashMap, sync::Arc};

use actix::{Actor, Addr, AsyncContext, Context, Handler, Message, StreamHandler};
use actix_cors::Cors;
use actix_web::{
    self, App, HttpRequest, HttpResponse, HttpServer, Responder, Result,
    web::{self, Data, Query},
};
use actix_web_actors::ws;
use rheomesh::config::{MediaConfig, WebRTCTransportConfig};
use rheomesh::publish_transport::PublishTransport;
use rheomesh::signaling::whip::{PublishTransportProvider, WhipEndpoint};
use rheomesh::subscribe_transport::SubscribeTransport;
use rheomesh::subscriber::Subscriber;
use rheomesh::transport::Transport;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing_actix_web::TracingLogger;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;

mod common;
use common::room::{Room, RoomOwner};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

#[derive(Clone)]
struct SessionStore {
    sessions: Arc<Mutex<HashMap<String, Addr<Session>>>>,
    owner: Arc<Mutex<RoomOwner<Session>>>,
}

struct Session {
    session_id: String,
    room: Arc<Room<Self>>,
    owner: Arc<Mutex<RoomOwner<Session>>>,
    publish_transport: Arc<PublishTransport>,
}

#[async_trait::async_trait]
impl PublishTransportProvider for SessionStore {
    async fn get_publish_transport(
        &self,
        session_id: &str,
    ) -> Result<Arc<PublishTransport>, actix_web::Error> {
        let sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(session_id) {
            let p = session.send(GetPublishTransport).await.map_err(|e| {
                actix_web::error::ErrorInternalServerError(format!(
                    "Failed to get publish transport: {}",
                    e
                ))
            })?;
            Ok(p)
        } else {
            Err(actix_web::error::ErrorNotFound("Session not found"))
        }
    }
}

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
    let room_owner = RoomOwner::<Session>::new(worker);
    let store = SessionStore {
        sessions: Arc::new(Mutex::new(HashMap::new())),
        owner: Arc::new(Mutex::new(room_owner)),
    };
    let store_data = Data::new(store.clone());

    let endpoint = WhipEndpoint::new(store);

    HttpServer::new(move || {
        // For local development, do not use for production.
        let cors = Cors::permissive();
        App::new()
            .wrap(TracingLogger::default())
            .wrap(cors)
            .app_data(store_data.clone())
            .service(index)
            .service(join_room)
            .route("/socket", web::get().to(socket))
            .configure(|cfg| {
                endpoint.clone().configure(cfg);
            })
    })
    .bind("0.0.0.0:4000")?
    .run()
    .await
}

#[actix_web::get("/")]
async fn index() -> impl Responder {
    HttpResponse::Ok().body("healthy")
}

#[actix_web::post("/rooms/{room_id}/join")]
async fn join_room(
    room_id: web::Path<String>,
    session_store: Data<SessionStore>,
) -> Result<impl Responder> {
    let room_owner = session_store.owner.clone();
    let find = room_owner
        .as_ref()
        .lock()
        .await
        .find_by_id(room_id.to_string());

    let media_config = MediaConfig::default();
    let session_id = uuid::Uuid::new_v4().to_string();

    let mut config = WebRTCTransportConfig::default();
    let ip = env::var("PUBLIC_IP").expect("PUBLIC_IP must be set");
    let ipv4 = ip
        .parse::<Ipv4Addr>()
        .expect("failed to parse public IP address");
    config.announced_ips = vec![IpAddr::V4(ipv4)];
    config.configuration.ice_servers = vec![RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_owned()],
        ..Default::default()
    }];

    match find {
        Some(room) => {
            tracing::info!("Room found, so joining it: {}", room_id);
            let router = room.router.clone();
            let publish_transport = router
                .lock()
                .await
                .create_publish_transport(config.clone())
                .await;
            let session = Session {
                session_id: session_id.clone(),
                owner: room_owner.clone(),
                room: room.clone(),
                publish_transport: Arc::new(publish_transport),
            };
            let addr = session.start();
            session_store
                .sessions
                .lock()
                .await
                .insert(session_id.clone(), addr);
        }
        None => {
            let owner = room_owner.clone();
            let mut owner = owner.lock().await;
            let (room, _router_id) = owner
                .create_new_room(room_id.to_string(), media_config)
                .await;
            let router = room.router.clone();
            let publish_transport = router
                .lock()
                .await
                .create_publish_transport(config.clone())
                .await;
            let session = Session {
                session_id: session_id.clone(),
                owner: room_owner.clone(),
                room: room.clone(),
                publish_transport: Arc::new(publish_transport),
            };
            let addr = session.start();
            session_store
                .sessions
                .lock()
                .await
                .insert(session_id.clone(), addr);
            tracing::info!("Room not found, so created a new one: {}", room.id);
        }
    }

    let response = JoinRoom { session_id };

    Ok(web::Json(response))
}

async fn socket(
    req: HttpRequest,
    session_store: Data<SessionStore>,
    stream: web::Payload,
) -> impl Responder {
    let query = req.query_string();

    let parameters =
        Query::<HashMap<String, String>>::from_query(query).expect("Failed to parse query");
    let room_id = parameters.get("room").expect("room is required");
    let room_owner = session_store.owner.clone();
    let find = room_owner
        .as_ref()
        .lock()
        .await
        .find_by_id(room_id.to_string());

    let media_config = MediaConfig::default();

    match find {
        Some(room) => {
            tracing::info!("Room found, so joining it: {}", room_id);
            let server = WebSocket::new(room).await;
            ws::start(server, &req, stream)
        }
        None => {
            let owner = room_owner.clone();
            let mut owner = owner.lock().await;
            let (room, _router_id) = owner
                .create_new_room(room_id.to_string(), media_config)
                .await;
            let server = WebSocket::new(room).await;
            ws::start(server, &req, stream)
        }
    }
}

struct WebSocket {
    subscribe_transport: Arc<SubscribeTransport>,
    room: Arc<Room<Session>>,
    subscribers: Arc<Mutex<HashMap<String, Arc<Mutex<Subscriber>>>>>,
}

impl WebSocket {
    pub async fn new(room: Arc<Room<Session>>) -> Self {
        tracing::info!("Starting WebSocket");
        let r = room.router.clone();
        let router = r.lock().await;

        let mut config = WebRTCTransportConfig::default();
        let ip = env::var("PUBLIC_IP").expect("PUBLIC_IP must be set");
        let ipv4 = ip
            .parse::<Ipv4Addr>()
            .expect("failed to parse public IP address");
        config.announced_ips = vec![IpAddr::V4(ipv4)];
        config.configuration.ice_servers = vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }];

        let subscribe_transport = router.create_subscribe_transport(config).await;

        Self {
            room,
            subscribe_transport: Arc::new(subscribe_transport),
            subscribers: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Actor for WebSocket {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        tracing::info!("New WebSocket connection is started");
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        tracing::info!("The WebSocket connection is stopped");
        let subscribe_transport = self.subscribe_transport.clone();
        actix::spawn(async move {
            subscribe_transport
                .close()
                .await
                .expect("failed to close subscribe_transport");
        });
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
                    tracing::info!("router {} publisher ids {:#?}", router.id, ids);
                    address.do_send(SendingMessage::Published { publisher_ids: ids });
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
            ReceivedMessage::Answer { sdp } => {
                let subscribe_transport = self.subscribe_transport.clone();
                actix::spawn(async move {
                    let _ = subscribe_transport
                        .set_answer(sdp)
                        .await
                        .expect("failed to set answer");
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

#[derive(Serialize)]
struct JoinRoom {
    session_id: String,
}

impl Actor for Session {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        let address = ctx.address();
        self.room.add_user(address);
        tracing::info!("Session started id: {}", self.session_id);
    }

    fn stopped(&mut self, ctx: &mut Context<Self>) {
        tracing::info!("Session stopped id: {}", self.session_id);
        let address = ctx.address();
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

impl Handler<GetPublishTransport> for Session {
    type Result = Arc<PublishTransport>;
    fn handle(&mut self, _msg: GetPublishTransport, _ctx: &mut Self::Context) -> Self::Result {
        self.publish_transport.clone()
    }
}

#[derive(Message)]
#[rtype(result = "Arc<PublishTransport>")]
struct GetPublishTransport;

#[derive(Deserialize, Message, Debug)]
#[serde(tag = "action")]
#[rtype(result = "()")]
enum ReceivedMessage {
    #[serde(rename_all = "camelCase")]
    Ping,
    #[serde(rename_all = "camelCase")]
    SubscriberInit,
    #[serde(rename_all = "camelCase")]
    SubscriberIce { candidate: RTCIceCandidateInit },
    #[serde(rename_all = "camelCase")]
    Answer { sdp: RTCSessionDescription },
    #[serde(rename_all = "camelCase")]
    Subscribe { publisher_id: String },
}

#[derive(Serialize, Message, Debug)]
#[serde(tag = "action")]
#[rtype(result = "()")]
enum SendingMessage {
    #[serde(rename_all = "camelCase")]
    Pong,
    #[serde(rename_all = "camelCase")]
    Offer { sdp: RTCSessionDescription },
    #[serde(rename_all = "camelCase")]
    SubscriberIce { candidate: RTCIceCandidateInit },
    #[serde(rename_all = "camelCase")]
    Published { publisher_ids: Vec<String> },
    #[serde(rename_all = "camelCase")]
    Subscribed { subscriber_id: String },
}

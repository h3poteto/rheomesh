use std::collections::HashMap;
use std::env;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use actix::{Actor, Addr, Message, StreamHandler};
use actix::{AsyncContext, Handler};
use actix_web::web::{Data, Query};
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use actix_web_actors::ws;
use futures_util::StreamExt;
use redis::{AsyncCommands, Commands};
use rheomesh::config::MediaConfig;
use rheomesh::publisher::Publisher;
use rheomesh::subscriber::Subscriber;
use rheomesh::transport::Transport;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing_actix_web::TracingLogger;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let worker = rheomesh::worker::Worker::new(rheomesh::config::WorkerConfig {
        relay_server_tcp_port: env::var("RELAY_SERVER_TCP_PORT")
            .unwrap_or_else(|_| "9443".to_string())
            .parse()
            .expect("Failed to parse RELAY_SERVER_TCP_PORT"),
        private_ip: env::var("LOCAL_IP")
            .expect("LOCAL_IP is required")
            .parse()
            .expect("Failed to parse LOCAL_IP"),
    })
    .await
    .unwrap();
    let room_owner = RoomOwner::new(worker);
    let room_data = Data::new(Mutex::new(room_owner));

    let port = env::var("PORT").unwrap_or_else(|_| "4000".to_string());

    HttpServer::new(move || {
        App::new()
            .wrap(TracingLogger::default())
            .service(index)
            .app_data(room_data.clone())
            .route("/socket", web::get().to(socket))
    })
    .bind(format!("0.0.0.0:{}", port))?
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

    let config = MediaConfig::default();

    match find {
        Some(room) => {
            tracing::info!("Room found, so joining it: {}", room_id);
            let server = WebSocket::new(room, room_owner.clone()).await;
            ws::start(server, &req, stream)
        }
        None => {
            let owner = room_owner.clone();
            let mut owner = owner.lock().await;
            let (room, router_id) = owner.create_new_room(room_id.to_string(), config).await;
            {
                store_room(
                    router_id,
                    room_id.to_string(),
                    env::var("LOCAL_IP").unwrap(),
                    env::var("RELAY_SERVER_TCP_PORT")
                        .unwrap_or_else(|_| "9443".to_string())
                        .parse()
                        .unwrap(),
                );
            }
            let server = WebSocket::new(room, room_owner.clone()).await;
            ws::start(server, &req, stream)
        }
    }
}

fn store_room(router_id: String, room_id: String, ip: String, port: u16) {
    // We are using redis to communicate between servers.
    // When you create your own server, you can use any communication method you like.
    // For example, WebSocket, gRPC, etc.
    let redis_host = env::var("REDIS_HOST").unwrap();
    let client = redis::Client::open(format!("redis://{}/", redis_host)).unwrap();
    let mut conn = client.get_connection().unwrap();
    let field = format!("{}|{}", ip, port);
    // hset overwrite the value if it exists.
    let _: () = conn.hset(room_id, field, router_id).unwrap();
}

fn delete_room(room_id: String, ip: String, port: u16) {
    let redis_host = env::var("REDIS_HOST").unwrap();
    let client = redis::Client::open(format!("redis://{}/", redis_host)).unwrap();
    let mut conn = client.get_connection().unwrap();
    let field = format!("{}|{}", ip, port);
    // hdel delete the value if it exists.
    let _: () = conn.hdel(room_id, field).unwrap();
}

fn get_pair_servers(
    room_id: String,
    my_ip: String,
    my_port: u16,
) -> HashMap<(String, u16), String> {
    let redis_host = env::var("REDIS_HOST").unwrap();
    let client = redis::Client::open(format!("redis://{}/", redis_host)).unwrap();
    let mut conn = client.get_connection().unwrap();
    let mut res: HashMap<String, String> = conn.hgetall(room_id).unwrap();
    let field = format!("{}|{}", my_ip, my_port);
    let _ = res.remove(&field);
    let parsed: HashMap<(String, u16), String> = res
        .into_iter()
        .filter_map(|(key, value)| {
            let parts: Vec<&str> = key.split('|').collect();
            if parts.len() == 2 {
                let ip = parts[0].to_string();
                let port = parts[1].parse::<u16>().unwrap_or(0);
                Some(((ip, port), value))
            } else {
                None
            }
        })
        .collect();
    parsed
}

struct WebSocket {
    owner: Data<Mutex<RoomOwner>>,
    room: Arc<Room>,
    publish_transport: Arc<rheomesh::publish_transport::PublishTransport>,
    subscribe_transport: Arc<rheomesh::subscribe_transport::SubscribeTransport>,
    publishers: Arc<Mutex<HashMap<String, Arc<Mutex<Publisher>>>>>,
    subscribers: Arc<Mutex<HashMap<String, Arc<Mutex<Subscriber>>>>>,
    redis_client: redis::Client,
    cancel: CancellationToken,
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
            min: env::var("RTC_MIN_PORT").unwrap().parse().unwrap(),
            max: env::var("RTC_MAX_PORT").unwrap().parse().unwrap(),
        });

        let publish_transport = router.create_publish_transport(config.clone()).await;
        let subscribe_transport = router.create_subscribe_transport(config).await;
        let redis_host = env::var("REDIS_HOST").unwrap();
        let client = redis::Client::open(format!("redis://{}/", redis_host)).unwrap();
        let cancel = CancellationToken::new();

        Self {
            owner,
            room,
            publish_transport: Arc::new(publish_transport),
            subscribe_transport: Arc::new(subscribe_transport),
            publishers: Arc::new(Mutex::new(HashMap::new())),
            subscribers: Arc::new(Mutex::new(HashMap::new())),
            redis_client: client,
            cancel,
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
        self.cancel.cancel();
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
                delete_room(
                    room_id.clone(),
                    env::var("LOCAL_IP").unwrap(),
                    env::var("RELAY_SERVER_TCP_PORT")
                        .unwrap_or_else(|_| "9443".to_string())
                        .parse()
                        .unwrap(),
                );
                let mut owner = owner.lock().await;
                owner.remove_room(room_id.clone());
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

                let redis_client = self.redis_client.clone();
                let channel = format!("{}_join", self.room.id.clone());
                let ip = env::var("LOCAL_IP").unwrap();
                let port = env::var("RELAY_SERVER_TCP_PORT")
                    .unwrap_or_else(|_| "9443".to_string())
                    .parse::<u16>()
                    .unwrap();
                let publishers = self.publishers.clone();
                let room_id = self.room.id.clone();
                let cancel_child = self.cancel.child_token();
                tokio::spawn(async move {
                    let mut conn = redis_client.get_async_pubsub().await.unwrap();
                    conn.subscribe(channel).await.unwrap();

                    let mut stream = conn.on_message();
                    loop {
                        tokio::select! {
                            _ = cancel_child.cancelled() => {
                                tracing::debug!("cancelled publisherInit");
                                break;
                            }
                            Some(msg) = stream.next() => {
                                let message = msg.get_payload::<String>().unwrap();
                                let value: serde_json::Value = serde_json::from_str(&message).unwrap();
                                tracing::debug!("subscriber added: {:#?}", value);
                                let target_ip = value.get("ip").unwrap().as_str().unwrap();
                                let pp = value.get("port").unwrap().as_u64().unwrap();
                                let target_port = u16::try_from(pp).unwrap();
                                let target_router_id = value.get("router_id").unwrap().as_str().unwrap();
                                if target_ip.to_string() != ip || target_port != port {
                                    let mut con = redis_client
                                        .get_multiplexed_async_connection()
                                        .await
                                        .unwrap();

                                    let guard = publishers.lock().await;
                                    for (track_id, publisher) in guard.iter() {
                                        let mut publisher = publisher.lock().await;
                                        let res = publisher
                                            .relay_to(target_ip.to_string(),target_port, target_router_id.to_string())
                                            .await
                                            .unwrap();
                                        if !res {
                                            tracing::error!("failed to relay publisher");
                                            continue;
                                        }
                                        let _ = con
                                            .publish::<String, String, i64>(
                                                room_id.clone(),
                                                track_id.clone(),
                                            )
                                            .await
                                            .unwrap();
                                        tracing::debug!(
                                            "relayed publisher id={} to router={} in {}",
                                            track_id,
                                            target_router_id,
                                            target_ip
                                        );
                                    }
                                }
                            }
                        }
                    }
                });
            }
            ReceivedMessage::SubscriberInit => {
                let subscribe_transport = self.subscribe_transport.clone();
                let room = self.room.clone();
                {
                    let address = address.clone();
                    tokio::spawn(async move {
                        let addr1 = address.clone();
                        let addr2 = address.clone();
                        subscribe_transport
                            .on_ice_candidate(Box::new(move |candidate| {
                                let init = candidate.to_json().expect("failed to parse candidate");
                                addr1.do_send(SendingMessage::SubscriberIce { candidate: init });
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

                let redis_client = self.redis_client.clone();
                let channel = format!("{}_join", self.room.id.clone());
                let router = self.room.router.clone();
                let ip = env::var("LOCAL_IP").unwrap();
                let port = env::var("RELAY_SERVER_TCP_PORT")
                    .unwrap_or_else(|_| "9443".to_string())
                    .parse::<u16>()
                    .unwrap();
                tokio::spawn(async move {
                    let router = router.lock().await;
                    let mut con = redis_client
                        .get_multiplexed_async_connection()
                        .await
                        .unwrap();

                    let json_value = json!({
                        "ip": ip,
                        "port": port,
                        "router_id": router.id
                    });
                    tracing::debug!("subscriberInit: {:#?}", json_value);
                    let json_string = serde_json::to_string(&json_value).unwrap();
                    let _ = con
                        .publish::<String, String, i64>(channel, json_string)
                        .await
                        .unwrap();
                });

                let redis_client = self.redis_client.clone();
                let channel = self.room.id.clone();
                let address = address.clone();
                let publishers = self.publishers.clone();
                let child_token = self.cancel.child_token();
                tokio::spawn(async move {
                    let mut conn = redis_client.get_async_pubsub().await.unwrap();
                    conn.subscribe(channel).await.unwrap();

                    let mut stream = conn.on_message();
                    loop {
                        tokio::select! {
                            _ = child_token.cancelled() => {
                                tracing::debug!("cancelled subscriberInit");
                                break;
                            }
                            Some(msg) = stream.next() => {
                                let publisher_id = msg.get_payload::<String>().unwrap();
                                let guard = publishers.lock().await;

                                if let None = guard.get(&publisher_id) {
                                    tracing::info!("relayed publisher received id={}", publisher_id);
                                    address.do_send(SendingMessage::Published {
                                        publisher_ids: vec![publisher_id],
                                    });
                                }
                            }
                        }
                    }
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
                let room_id = room.id.clone();
                let redis_client = self.redis_client.clone();
                let channel = self.room.id.clone();
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
                            {
                                let mut p = publishers.lock().await;
                                p.insert(track_id.clone(), publisher.clone());
                                room.get_peers(&address).iter().for_each(|peer| {
                                    peer.do_send(SendingMessage::Published {
                                        publisher_ids: vec![track_id.clone()],
                                    });
                                });
                            }

                            let servers = get_pair_servers(
                                room_id,
                                env::var("LOCAL_IP").unwrap(),
                                env::var("RELAY_SERVER_TCP_PORT")
                                    .unwrap_or_else(|_| "9443".to_string())
                                    .parse()
                                    .unwrap(),
                            );
                            let mut con = redis_client
                                .get_multiplexed_async_connection()
                                .await
                                .unwrap();
                            for ((ip, port), router_id) in servers {
                                let mut publisher = publisher.lock().await;
                                let _ = publisher
                                    .relay_to(ip, port, router_id.clone())
                                    .await
                                    .unwrap();
                                let _ = con
                                    .publish::<String, String, i64>(
                                        channel.clone(),
                                        track_id.clone(),
                                    )
                                    .await
                                    .unwrap();
                            }
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
    ) -> (Arc<Room>, String) {
        let mut worker = self.worker.lock().await;
        let router = worker.new_router(config);
        #[allow(unused)]
        let mut router_id = "".to_string();
        {
            let locked = router.lock().await;
            router_id = locked.id.clone();
        }
        let room = Room::new(id.clone(), router);
        let a = Arc::new(room);
        self.rooms.insert(id.clone(), a.clone());
        (a, router_id)
    }

    fn remove_room(&mut self, room_id: String) {
        self.rooms.remove(&room_id);
    }
}

struct Room {
    pub id: String,
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

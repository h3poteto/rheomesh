use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use actix::{Actor, Addr, AsyncContext, Handler, Message, StreamHandler};
use actix_web::web::{Data, Query};
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use actix_web_actors::ws;
use rheomesh::config::MediaConfig;
use rheomesh::data_publisher::DataPublisher;
use rheomesh::data_subscriber::DataSubscriber;
use rheomesh::transport::Transport;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
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

    let room_owner = RoomOwner::new();
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

    let config = MediaConfig::default();

    match find {
        Some(room) => {
            tracing::info!("Room found, so joining it: {}", room_id);
            let server = WebSocket::new(room).await;
            ws::start(server, &req, stream)
        }
        None => {
            let owner = room_owner.clone();
            let mut owner = owner.lock().await;
            let router = rheomesh::router::Router::new(config);
            let room = owner.create_new_room(room_id.to_string(), router).await;
            let server = WebSocket::new(room).await;
            ws::start(server, &req, stream)
        }
    }
}

struct WebSocket {
    room: Arc<Room>,
    publish_transport: Arc<rheomesh::publish_transport::PublishTransport>,
    subscribe_transport: Arc<rheomesh::subscribe_transport::SubscribeTransport>,
    data_publishers: Arc<Mutex<HashMap<String, Arc<DataPublisher>>>>,
    data_subscribers: Arc<Mutex<HashMap<String, Arc<DataSubscriber>>>>,
}

impl WebSocket {
    pub async fn new(room: Arc<Room>) -> Self {
        tracing::info!("Starting WebSocket");
        let r = room.router.clone();
        let router = r.lock().await;

        let mut config = rheomesh::config::WebRTCTransportConfig::default();
        // Public IP address of your server.
        config.announced_ips = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 10, 10))];
        config.configuration.ice_servers = vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }];

        let publish_transport = router.create_publish_transport(config.clone()).await;
        let subscribe_transport = router.create_subscribe_transport(config).await;

        Self {
            room,
            publish_transport: Arc::new(publish_transport),
            subscribe_transport,
            data_publishers: Arc::new(Mutex::new(HashMap::new())),
            data_subscribers: Arc::new(Mutex::new(HashMap::new())),
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
        self.room.remove_user(address);
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
                Err(err) => {
                    tracing::error!("failed to parse client message: {}\n{}", err, text);
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
                    let ids = router.track_ids();
                    tracing::info!("router track ids {:#?}", ids);
                    ids.iter().for_each(|id| {
                        address.do_send(SendingMessage::Published {
                            publisher_id: id.to_string(),
                        });
                    });
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
            ReceivedMessage::Subscribe {
                publisher_id: channel_id,
            } => {
                let subscribe_transport = self.subscribe_transport.clone();
                let subscribers = self.data_subscribers.clone();
                actix::spawn(async move {
                    let (subscriber, offer) = subscribe_transport
                        .data_subscribe(channel_id)
                        .await
                        .expect("failed to connect subscribe_transport");

                    let id = subscriber.id.clone();
                    let mut s = subscribers.lock().await;
                    s.insert(subscriber.id.clone(), Arc::new(subscriber));
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
            ReceivedMessage::Publish { channel_id } => {
                let room = self.room.clone();
                let publish_transport = self.publish_transport.clone();
                let publishers = self.data_publishers.clone();
                actix::spawn(async move {
                    match publish_transport.data_publish(channel_id).await {
                        Ok(publisher) => {
                            tracing::debug!("published a data channel: {}", publisher.id);
                            // address.do_send(SendingMessage::Published {
                            //     track_id: id.clone(),
                            // });
                            let mut p = publishers.lock().await;
                            p.insert(publisher.id.clone(), publisher.clone());
                            room.get_peers(&address).iter().for_each(|peer| {
                                peer.do_send(SendingMessage::Published {
                                    publisher_id: publisher.id.clone(),
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
                let publishers = self.data_publishers.clone();
                actix::spawn(async move {
                    let mut p = publishers.lock().await;
                    if let Some(publisher) = p.remove(&publisher_id) {
                        publisher.close().await;
                    }
                });
            }
            ReceivedMessage::StopSubscribe { subscriber_id } => {
                let subscribers = self.data_subscribers.clone();
                actix::spawn(async move {
                    let mut s = subscribers.lock().await;
                    if let Some(subscriber) = s.remove(&subscriber_id) {
                        subscriber.close().await;
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
    Publish { channel_id: String },
    #[serde(rename_all = "camelCase")]
    StopPublish { publisher_id: String },
    #[serde(rename_all = "camelCase")]
    StopSubscribe { subscriber_id: String },
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
    Published { publisher_id: String },
    #[serde(rename_all = "camelCase")]
    Subscribed { subscriber_id: String },
}

struct RoomOwner {
    rooms: HashMap<String, Arc<Room>>,
}

impl RoomOwner {
    pub fn new() -> Self {
        RoomOwner {
            rooms: HashMap::<String, Arc<Room>>::new(),
        }
    }

    fn find_by_id(&self, id: String) -> Option<Arc<Room>> {
        self.rooms.get(&id).cloned()
    }

    async fn create_new_room(
        &mut self,
        id: String,
        router: Arc<Mutex<rheomesh::router::Router>>,
    ) -> Arc<Room> {
        let room = Room::new(id.clone(), router);
        let a = Arc::new(room);
        self.rooms.insert(id.clone(), a.clone());
        a
    }
}

struct Room {
    _id: String,
    pub router: Arc<Mutex<rheomesh::router::Router>>,
    users: std::sync::Mutex<Vec<Addr<WebSocket>>>,
}

impl Room {
    pub fn new(_id: String, router: Arc<Mutex<rheomesh::router::Router>>) -> Self {
        Self {
            _id,
            router,
            users: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn add_user(&self, user: Addr<WebSocket>) {
        let mut users = self.users.lock().unwrap();
        users.push(user);
    }

    pub fn remove_user(&self, user: Addr<WebSocket>) {
        let mut users = self.users.lock().unwrap();
        users.retain(|u| u != &user);
    }

    pub fn get_peers(&self, user: &Addr<WebSocket>) -> Vec<Addr<WebSocket>> {
        let users = self.users.lock().unwrap();
        users.iter().filter(|u| u != &user).cloned().collect()
    }
}

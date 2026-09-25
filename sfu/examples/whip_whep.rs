use std::env;
use std::net::{IpAddr, Ipv4Addr};
use std::{collections::HashMap, sync::Arc};

use actix::{Actor, Addr, AsyncContext, Context, Handler, Message};
use actix_cors::Cors;
use actix_web::http::header;
use actix_web::{
    self, App, HttpResponse, HttpServer, Responder, Result,
    web::{self, Data},
};

use rheomesh::config::{MediaConfig, WebRTCTransportConfig};
use rheomesh::publish_transport::PublishTransport;
use rheomesh::signaling::whep::{SubscribeTransportProvider, WhepEndpoint};
use rheomesh::signaling::whip::{PublishTransportProvider, WhipEndpoint};
use rheomesh::subscribe_transport::SubscribeTransport;
use serde::Serialize;
use tokio::sync::Mutex;
use tracing_actix_web::TracingLogger;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use webrtc::peer_connection::{RTCConfigurationBuilder, RTCIceServer};

mod common;
use common::room::{Room, RoomOwner};

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
    subscribe_transport: Arc<SubscribeTransport>,
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

#[async_trait::async_trait]
impl SubscribeTransportProvider for SessionStore {
    async fn get_subscribe_transport(
        &self,
        session_id: &str,
    ) -> Result<Arc<SubscribeTransport>, actix_web::Error> {
        let sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(session_id) {
            let p = session.send(GetSubscribeTransport).await.map_err(|e| {
                actix_web::error::ErrorInternalServerError(format!(
                    "Failed to get subscribe transport: {}",
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

    let whip = WhipEndpoint::new(store.clone());
    let whep = WhepEndpoint::new(store);

    HttpServer::new(move || {
        let cors = Cors::default()
            .allowed_origin("http://localhost:3000")
            .allowed_methods(vec!["GET", "POST", "PATCH", "DELETE", "OPTION"])
            .allowed_headers(vec![
                header::AUTHORIZATION,
                header::ACCEPT,
                header::CONTENT_TYPE,
                header::IF_MATCH,
            ])
            .expose_headers(vec![header::LINK, header::LOCATION, header::ETAG]);
        App::new()
            .wrap(TracingLogger::default())
            .wrap(cors)
            .app_data(store_data.clone())
            .service(index)
            .service(join_room)
            .configure(|cfg| {
                whip.clone().configure(cfg);
                whep.clone().configure(cfg);
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
    config.configuration = RTCConfigurationBuilder::new()
        .with_ice_servers(vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }])
        .build();

    let mut publisher_ids = vec![];

    match find {
        Some(room) => {
            tracing::info!("Room found, so joining it: {}", room_id);
            let router = room.router.clone();
            let publish_transport = router
                .lock()
                .await
                .create_publish_transport(config.clone())
                .await
                .expect("failed to create publish_transport");
            let subscribe_transport = router
                .lock()
                .await
                .create_subscribe_transport(config.clone())
                .await
                .expect("failed to create subscribe_transport");
            let session = Session {
                session_id: session_id.clone(),
                owner: room_owner.clone(),
                room: room.clone(),
                publish_transport: Arc::new(publish_transport),
                subscribe_transport: Arc::new(subscribe_transport),
            };
            let addr = session.start();
            session_store
                .sessions
                .lock()
                .await
                .insert(session_id.clone(), addr);

            publisher_ids = router.lock().await.publisher_ids();
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
                .await
                .expect("failed to create publish_transport");
            let subscribe_transport = router
                .lock()
                .await
                .create_subscribe_transport(config.clone())
                .await
                .expect("failed to create subscribe_transport");
            let session = Session {
                session_id: session_id.clone(),
                owner: room_owner.clone(),
                room: room.clone(),
                publish_transport: Arc::new(publish_transport),
                subscribe_transport: Arc::new(subscribe_transport),
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

    let response = JoinRoom {
        session_id,
        publisher_ids,
    };

    Ok(web::Json(response))
}

#[derive(Serialize)]
struct JoinRoom {
    session_id: String,
    publisher_ids: Vec<String>,
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

impl Handler<GetSubscribeTransport> for Session {
    type Result = Arc<SubscribeTransport>;
    fn handle(&mut self, _msg: GetSubscribeTransport, _ctx: &mut Self::Context) -> Self::Result {
        self.subscribe_transport.clone()
    }
}

#[derive(Message)]
#[rtype(result = "Arc<PublishTransport>")]
struct GetPublishTransport;

#[derive(Message)]
#[rtype(result = "Arc<SubscribeTransport>")]
struct GetSubscribeTransport;

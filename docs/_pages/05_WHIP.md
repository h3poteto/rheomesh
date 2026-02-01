---
title: WebRTC-HTTP Ingestion Protocol (WHIP)
layout: post
---

[WebRTC-HTTP Ingestion Protocol(WHIP)](https://www.rfc-editor.org/rfc/rfc9725.html) is a protocol to support WebRTC ingestion signal on HTTP. Rheomesh supports this protocol.

However, currently Rheomesh supports only [actix](https://actix.rs/) web servers for WHIP.


# Overview

WHIP provides a standardized HTTP-based signaling protocol for WebRTC ingestion. Unlike the custom signaling approach described in [Getting Started]({{ site.baseurl }}/pages/02_getting_started/), WHIP uses
standard HTTP endpoints, making it easier to integrate with existing infrastructure and tools.

**When to use WHIP:**
- You want standards-based signaling without implementing custom WebSocket or HTTP protocols
- You're building a broadcast/ingestion workflow (e.g., OBS, FFmpeg streaming to your SFU)
- You need compatibility with WHIP-compatible clients and tools

**When to use custom signaling (Getting Started approach):**
- You need bidirectional communication (both publish and subscribe in the same session)
- You want full control over the signaling protocol
- You're building an interactive application (video conferencing, collaborative tools)

# How WHIP works with Rheomesh

WHIP handles only the **publish** side of WebRTC signaling. The protocol defines three HTTP endpoints:

- **POST** `/whip/{session_id}` - Accepts SDP offer from client, returns SDP answer
- **PATCH** `/whip/{session_id}` - Handles trickle ICE candidates
- **DELETE** `/whip/{session_id}` - Terminates the session

The `session_id` is a unique identifier for each publishing session. This could be:
- A user ID (e.g., "user123")
- A stream key (e.g., "live-stream-abc")
- A room and user combination (e.g., "room1-user456")

You decide what the session_id represents based on your application's needs. The session_id is used to:
1. Route the WHIP request to the correct PublishTransport
2. Identify which session to update or terminate

# Prepare PublishTransportProvider for sessions
Rheomesh doesn't control which [PublishTransport](https://docs.rs/rheomesh/latest/rheomesh/publish_transport/struct.PublishTransport.html) a user should use. So, please implement a struct and method to connect a user to a PublishTransport.

The `PublishTransportProvider` trait acts as a bridge between WHIP endpoints (which receive session_id from HTTP requests) and your PublishTransport instances (which handle the actual WebRTC connections).

```rust
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use rheomesh::publish_transport::PublishTransport;
use rheomesh::signaling::whip::PublishTransportProvider;

struct SessionStore {
    sessions: Arc<Mutex<HashMap<String, Session>>>
}

struct Session {
    session_id: String,  // Or user_id.
    publish_transport: Arc<PublishTransport>
}

#[async_trait]
impl PublishTransportProvider for SessionStore {
    async fn get_publish_transport(
        &self,
        session_id: &str,
    ) -> Result<Arc<PublishTransport>, actix_web::Error> {
        let sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(session_id) {
            let p = session.publish_transport.clone();
            Ok(p)
        } else {
            Err(actix_web::error::ErrorNotFound("Session not found"))
        }
    }
}
```

**Important**: You must create and register PublishTransport instances in your SessionStore **before** clients make WHIP requests.

# Inject WHIP endpoints
```rust
use actix_web::{App, HttpServer, web::Data};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use rheomesh::signaling::whip::WhipEndpoint;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let store = SessionStore {
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };
    let store_data = Data::new(store.clone());

    let endpoint = WhipEndpoint::new(store);

    HttpServer::new(move || {
        App::new()
            .app_data(store_data.clone())
            .configure(|cfg| {
                endpoint.clone().configure(cfg);
            })
    })
    .bind("0.0.0.0:4000")?
    .run()
    .await
}
```

This example provides these 3 endpoints for WHIP.

- POST `/whip/session_id` - For SDP offer
- PATCH `/whip/session_id` - For trickle ICE
- DELETE `/whip/session_id` - For ending the session

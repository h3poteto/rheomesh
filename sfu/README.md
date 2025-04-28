# Rheomesh
[![Build](https://github.com/h3poteto/rheomesh/actions/workflows/build.yml/badge.svg)](https://github.com/h3poteto/rheomesh/actions/workflows/build.yml)
[![docs.rs](https://img.shields.io/docsrs/rheomesh/latest)](https://docs.rs/rheomesh/latest/rheomesh/)
[![Crates.io](https://img.shields.io/crates/v/rheomesh)](https://crates.io/crates/rheomesh)
[![GitHub](https://img.shields.io/github/license/h3poteto/rheomesh)](https://github.com/h3poteto/rheomesh/LICENSE)

Rheomesh is a WebRTC SFU library that provides a simple API for building real-time communication applications. This provides an SDK to help you build a WebRTC SFU server.
[Here](https://github.com/h3poteto/rheomesh/blob/master/sfu/examples/media_server.rs) is an example SFU server for video streaming.

## Install
Add your `Cargo.toml` like this:
```
[dependencies]
rheomesh = { version = "0" }
```

## Usage
### Create router and transports
First of all, please create a worker and router. Worker accomodates multiple routers. You only need to launch one worker per server. Router accommodates multiple transports and they can communicate with each other. That means transports belonging to the same Router can send/receive their media. Router is like a meeting room.

```rust
use rheomesh::worker::Worker;
use rheomesh::config::{WorkerConfig};

//...

async fn new() {
  let worker = Worker::new(WorkerConfig::default()).await.unwrap();
  let config = MediaConfig::default();
  let mut w = worker.lock().await;
  let router = w.new_router(config);
}
```

Next, please create publish and subscribe transports.

```rust
use rheomesh::config::WebRTCTransportConfig

async fn new() {
  //...
  let mut config = WebRTCTransportConfig::default();
  config.configuration.ice_servers = vec![RTCIceServer {
    urls: vec!["stun:stun.l.google.com:19302".to_owned()],
    ..Default::default()
  }];
  let publish_transport = router.lock().await.create_publish_transport(config.clone()).await;
  let subscribe_transport = router.lock().await.create_subscribe_transport(config.clone()).await;
}
```

### Handle publish events
#### Bind `on_ice_candidate` callback
```rust
publish_transport
  .on_ice_candidate(Box::new(move |candidate| {
      let init = candidate.to_json().expect("failed to parse candidate");
      // Send `init` message to client. The client have to call `addIceCandidate` method with this parameter.
  }))
  .await;
```
Please send `init` to client. The corresponding client-side handler is [here](https://github.com/h3poteto/rheomesh/client/README.md#handle-rtcicecandidateinit).

#### Handle `RTCIceCandidateInit`
On the other hand, you will receive `RTCIceCandidateInit` message from client, [here](https://github.com/h3poteto/rheomesh/blob/master/client/README.md#bind-icecandidate-events).

```rust
let _ = publish_transport
  .add_ice_candidate(candidate)
  .await
  .expect("failed to add ICE candidate");
```
#### Handle `offer` message
Then, server will receive `offer` from client, the corresponding client-side handler is [here](https://github.com/h3poteto/rheomesh/blob/master/client/README.md#publish).
```rust
let answer = publish_transport
  .get_answer(offer)
  .await
  .expect("failed to connect publish_transport");
// Send `answer` message to client. The client have to call `setAnswer` method.
```
Please send `answer` to client. The corresponding client-side handler is [here](https://github.com/h3poteto/rheomesh/blob/master/client/README.md#handle-answer-message).

#### Publish
Finally, please handle publish event with `track_id`.
```rust
let publisher = publish_transport.publish(track_id).await;
```

### Handle subscribe events
#### Bind `on_ice_candidate` and `on_negotiation_needed` callback
```rust
subscribe_transport
  .on_ice_candidate(Box::new(move |candidate| {
      let init = candidate.to_json().expect("failed to parse candidate");
      // Send `init` message to client. The client have to call `addIceCandidate` method with this parameter.
  }))
    .await;
subscribe_transport
  .on_negotiation_needed(Box::new(move |offer| {
    // Send `offer` message to client. The client have to call `setOffer` method.
  }))
```
Please send `init` to client. The corresponding client-side handler is [here](https://github.com/h3poteto/rheomesh/blob/master/client/README.md#handle-rtcicecandidateinit-1).

#### Handle `RTCIceCandidateInit`
On the other hand, you will receive `RTCIceCandidateInit` message from client, [here](https://github.com/h3poteto/rheomesh/blob/master/client/README.md#bind-icecandidate-events-1).

```rust
let _ = subscribe_transport
  .add_ice_candidate(candidate)
  .await
  .expect("failed to add ICE candidate");
```
#### Subscribe
Then, please call `subscribe` method.
```rust
let (subscriber, offer) = subscribe_transport
  .subscribe(track_id)
  .await
  .expect("failed to connect subscribe_transport");
// Send `offer` message to client. The client have to call `setOffer` method.
```
Please send `offer` message to client. The corresponding client-side handler is [here](https://github.com/h3poteto/rheomesh/blob/master/client/README.md#handle-offer-message).
#### Handle `answer` message
Finally, server will receive `answer` from client, the corresponding client-side handler is [here](https://github.com/h3poteto/rheomesh/blob/master/client/README.md#handle-offer-message).
```rust
let _ = subscribe_transport
  .set_answer(answer)
  .await
  .expect("failed to set answer");
```

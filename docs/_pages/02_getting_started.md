---
layout: post
title: Getting Started
---

# Architecture Overview

Rheomesh uses a hierarchical structure for managing media streaming:

- **Worker**: The top-level container that manages system resources. Create one Worker per server process.
- **Router**: A media routing unit that handles multiple transports. Think of it as a "meeting room" where participants can exchange media.
- **Transport**: Individual connections for publishing or subscribing to media streams.

```
Worker (one per server)
 └── Router (meeting room)
     ├── PublishTransport (sends media)
     ├── SubscribeTransport (receives media)
     └── RecordingTransport (records media)
```

# Create router and transports
## Server side
First, create a [Worker](https://docs.rs/rheomesh/latest/rheomesh/worker/struct.Worker.html) and [Router](https://docs.rs/rheomesh/latest/rheomesh/router/struct.Router.html). All media communication happens within the same Router.

```rust
use rheomesh::worker::Worker;
use rheomesh::config::{WorkerConfig, MediaConfig};

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
use rheomesh::config::WebRTCTransportConfig;
use webrtc::ice_transport::ice_server::RTCIceServer;

async fn new() {
  //...
  let mut config = WebRTCTransportConfig::default();
  config.configuration.ice_servers = vec![RTCIceServer {
    urls: vec!["stun:stun.l.google.com:19302".to_owned()],
    ..Default::default()
  }];
  let publish_transport = router.create_publish_transport(config.clone()).await;
  let subscribe_transport = router.create_subscribe_transport(config.clone()).await;
}
```

## Client side
Please create PublishTransport and SubscribeTransport in client side.

```typescript
import { PublishTransport, SubscribeTransport } from 'rheomesh'

const peerConnectionConfig: RTCConfiguration = {
  iceServers: [{ urls: "stun:stun.l.google.com:19302" }],
}

const publishTransport = new PublishTransport(peerConnectionConfig)
const subscribeTransport = new SubscribeTransport(peerConnectionConfig)
```


# Handle publish events
## ICE events
### Server side `on_ice_candidate`

Bind `on_ice_candidate` callback.

```rust
publish_transport
  .on_ice_candidate(Box::new(move |candidate| {
      let candidate_init = candidate.to_json().expect("failed to parse candidate");
      // Send `candidate_init` message to client. The client has to call `addIceCandidate` method with this parameter.
  }))
  .await;
```

### Client side
```typescript
publishTransport.addIceCandidate(candidateInit)
```

Next, bind `icecandidate` callback.

```typescript
publishTransport.on("icecandidate", (candidate) => {
  // Send `candidate` to server. The server has to call `add_ice_candidate` method with this parameter.
})
```

### Server side
```rust
let _ = publish_transport
  .add_ice_candidate(candidate)
  .await
  .expect("failed to add ICE candidate");
```

## Publish
### Client side
When you get stream, please publish it to the publish transport.
```typescript
const stream = await navigator.mediaDevices.getDisplayMedia({
  video: true,
  audio: false,
})
stream.getTracks().forEach(async (track) => {
  const publisher = await publishTransport.publish(track)
  const offer = publisher.offer
  // Send `offer` to server. The server has to call `get_answer` method with this parameter.
  const publisherId = publisher.id  // This ID uniquely identifies the publisher
  // Send `publisherId` to server. The server has to call `publish` method with this parameter.
})
```

### Server side
Then, call `get_answer` and `publish` methods.

```rust
let answer = publish_transport
  .get_answer(offer)
  .await
  .expect("failed to connect publish_transport");
// Send `answer` message to client. The client have to call `setAnswer` method.
```

```rust
// `publisher_id` is the same value as `publisherId` received from client
let publisher = publish_transport.publish(publisher_id).await;
// `publisher` contains the media track that can be used for subscription or recording
```

### Handle `answer` in client side
```typescript
publishTransport.setAnswer(answer)
```

Finally, the server receives the track from client side.


# Handle subscribe events
## ICE events
### Server side `on_ice_candidate`
Bind `on_ice_candidate` and `on_negotiation_needed` callbacks.
```rust
subscribe_transport
  .on_ice_candidate(Box::new(move |candidate| {
      let candidate_init = candidate.to_json().expect("failed to parse candidate");
      // Send `candidate_init` message to client. The client has to call `addIceCandidate` method with this parameter.
  }))
    .await;
subscribe_transport
  .on_negotiation_needed(Box::new(move |offer| {
    // Send `offer` message to client. The client has to call `setOffer` method.
  }))
  .await;
```

### Client side
```typescript
subscribeTransport.addIceCandidate(candidateInit)
```

Next, bind `icecandidate` callback.


```typescript
subscribeTransport.on("icecandidate", (candidate) => {
  // Send `candidate` to server. The server has to call `add_ice_candidate` method with this parameter.
})
```

### Server side
```rust
let _ = subscribe_transport
  .add_ice_candidate(candidate)
  .await
  .expect("failed to add ICE candidate");
```

# Subscribe
## Server side
When you get [track](https://docs.rs/rheomesh/latest/rheomesh/track/trait.Track.html) from the [publisher](https://docs.rs/rheomesh/latest/rheomesh/publisher/struct.Publisher.html), please call `subscribe` method.

```rust
// Get track_id from the publisher created above
let track_id = publisher.track().id();
let (subscriber, offer) = subscribe_transport
  .subscribe(track_id)
  .await
  .expect("failed to connect subscribe_transport");
// Send `offer` message to client. The client has to call `setOffer` method.
```

### Client side
```typescript
subscribeTransport.setOffer(offer).then((answer) => {
  // Send `answer` to server. The server has to call `set_answer` method with this parameter.
})
```

### Server side `answer`
```rust
let _ = subscribe_transport
  .set_answer(answer)
  .await
  .expect("failed to set answer");
```

### Client side `subscribe`
```typescript
// Use the same `publisherId` that was sent to server earlier
subscribeTransport.subscribe(publisherId).then((subscriber) => {
  const stream = new MediaStream([subscriber.track])
  remoteVideo.srcObject = stream
});
```

Eventually, you can receive the track on client side.

# Rheomesh
[![Build](https://github.com/h3poteto/rheomesh/actions/workflows/build.yml/badge.svg)](https://github.com/h3poteto/rheomesh/actions/workflows/build.yml)
[![NPM Version](https://img.shields.io/npm/v/rheomesh.svg)](https://www.npmjs.com/package/rheomesh)
[![npm](https://img.shields.io/npm/dm/rheomesh)](https://www.npmjs.com/package/rheomesh)
[![GitHub](https://img.shields.io/github/license/h3poteto/rheomesh)](https://github.com/h3poteto/rheomesh/LICENSE)


Rheomesh is a WebRTC SFU library that provides a simple API for building real-time communication applications. This provides client-side library for WebRTC SFU servers.
[Here](example/media) is an example SFU client for video streaming.


## Install
```
$ npm install -S rheomesh
```

## Usage
### Create transports
First of all, please create publish and subscribe transports.

```typescript
import { PublishTransport } from 'rheomesh'

const peerConnectionConfig: RTCConfiguration = {
  iceServers: [{ urls: "stun:stun.l.google.com:19302" }],
}

const publishTransport = new PublishTransport(peerConnectionConfig)
const subscribeTransport = new SubscribeTransport(peerConnectionConfig)
```

### Handle publish events
#### Bind `icecandidate` events
```typescript
publishTransport.on("icecandidate", (candidate) => {
  // Send `candidate` to server. The server have to call `add_ice_candidate` method with this parameter.
})
```
Please send `candidate` to server. The corresponding server-side handler is [here](/sfu/README.md#handle-rtcicecandidateinit).

#### Handle `RTCIceCandidateInit`
`candidateInit` will be come from [here](/sfu/README.md#bind-on_ice_candidate-callback).

```typescript
publishTransport.addIceCandidate(candidateInit)
```

#### Publish
When you get stream, please publish it to the publish transport.

```typescript
const stream = await navigator.mediaDevices.getDisplayMedia({
  video: true,
  audio: false,
})
stream.getTracks().forEach(async (track) => {
  const publisher = await publishTransport.publish(track)
  const offer = publisher.offer
  // Send `offer` to server. The server have to call `get_answer` method with this parameter.
})

```
Please send `offer` to server. The corresponding server-side handler is [here](/sfu/README.md#handle-offer-message).

#### Handle `answer` message
`answer` message will be come from [here](/sfu/README.md#handle-offer-message).
```typescript
publishTransport.setAnswer(answer)
```

### Handle subscribe events
#### Bind `icecandidate` events
```typescript
subscribeTransport.on("icecandidate", (candidate) => {
  // Send `candidate` to server. The server have to call `add_ice_candidate` method with this parameter.
})
```
Please send `candidate` to server. The corresponding server-side handler is [here](/sfu/README.md#handle-rtcicecandidateinit-1).

#### Handle `RTCIceCandidateInit`
`candidateInit` will be come from [here](/sfu/README.md#bind-on_ice_candidate-and-on_negotiation_needed-callback).
```typescript
subscribeTransport.addIceCandidate(candidateInit)
```

#### Handle `offer` message
`offer` message sill be come from [here](/sfu/README.md#subscribe).

```typescript
subscribeTransport.setOffer(offer).then((answer) => {
  // Send `answer` to server. The server have to call `set_answer` method with this parameter.
})
```
Please send `answer` to server. The corresponding server-side handler is [here](/sfu/README.md#handle-answer-message).

#### Subscribe
```typescript
subscribeTransport.subscribe(publisherId).then((subscriber) => {
  const stream = new MediaStream([subscriber.track])
  remoteVideo.srcObject = stream
});
```

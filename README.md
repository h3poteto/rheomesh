# Rheomesh
[![E2E](https://github.com/h3poteto/rheomesh/actions/workflows/e2e.yml/badge.svg?branch=master)](https://github.com/h3poteto/rheomesh/actions/workflows/e2e.yml)
[![Build](https://github.com/h3poteto/rheomesh/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/h3poteto/rheomesh/actions/workflows/build.yml)
[![Crates.io](https://img.shields.io/crates/v/rheomesh)](https://crates.io/crates/rheomesh)
[![NPM Version](https://img.shields.io/npm/v/rheomesh.svg)](https://www.npmjs.com/package/rheomesh)
[![GitHub release](https://img.shields.io/github/release/h3poteto/rheomesh.svg)](https://github.com/h3poteto/rheomesh/releases)
[![GitHub](https://img.shields.io/github/license/h3poteto/rheomesh)](LICENSE)

Rheomesh is a WebRTC SFU ([Selective Forwarding Unit](https://bloggeek.me/webrtcglossary/sfu/)) library written by Rust. This provides an SDK to help you build a WebRTC SFU server. And this provides client-side library with TypeScript.

## Features
- [x] Video and Audio streaming
- [x] Data channels
- [x] Simulcast
- [ ] Scalable Video Coding (SVC)
- [x] Relay
- [ ] Recording

## Architecture
```mermaid
graph LR
subgraph SFU
    W[Worker] --> R1[Router]
    W --> R2[Router]
    subgraph R1[Router]
        PS1[PublisherTransport]
        P1[Publisher]
        PS1 --> P1
        SS2[SubscriberTransport]
        S1[Subscriber]
        SS2 --> S1
        P1 -->|Track| S1
    end
end
subgraph Browser1
    PC1[PublisherTransport]
    T1[VideoTrack]
    T1 --> PC1
end
subgraph Browser2
    SC2[SubscriberTransport]
    T2[VideoTrack]
    SC2 --> T2
end
PC1 -->|RTP| PS1
SS2 -->|RTP| SC2

```

## Server-side
Please refer [server-side document](sfu).

## Client-side
Please refer [client-side documents](client).

## Example
### Server side
```
$ cd sfu
$ cargo run --example media_server
```

WebScoket signaling server will launch with `0.0.0.0:4000`.

### Client side
```
$ cd client
$ yarn install
$ yarn workspace media dev
```

You can access the frontend service with `localhost:5173`.

# License
The software is available as open source under the terms of the [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0).

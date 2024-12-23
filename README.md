# Rheomesh
[![Build](https://github.com/h3poteto/rheomesh/actions/workflows/build.yml/badge.svg)](https://github.com/h3poteto/rheomesh/actions/workflows/build.yml)
[![Crates.io](https://img.shields.io/crates/v/rheomesh)](https://crates.io/crates/rheomesh)
[![NPM Version](https://img.shields.io/npm/v/rheomesh.svg)](https://www.npmjs.com/package/rheomesh)
[![GitHub release](https://img.shields.io/github/release/h3poteto/rheomesh.svg)](https://github.com/h3poteto/rheomesh/releases)
[![GitHub](https://img.shields.io/github/license/h3poteto/rheomesh)](LICENSE)

Rheomesh is a WebRTC SFU ([Selective Forwarding Unit](https://bloggeek.me/webrtcglossary/sfu/)) library written by Rust. This provides an SDK to help you build a WebRTC SFU server. And this provides client-side library with TypeScript.

## Features
- [x] Video and Audio streaming
- [x] Data channels
- [ ] Simulcast
- [ ] Relay

## Architecture
![overview](./rheomesh.jpg)

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

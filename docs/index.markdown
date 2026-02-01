---
title: Rheomesh
layout: post
---

Rheomesh is a WebRTC SFU ([Selective Forwarding Unit](https://bloggeek.me/webrtcglossary/sfu/)) library written by Rust. This provides an SDK to help you build a WebRTC SFU server. And this provides client-side library with TypeScript.

This SDK supports following features.


- [x] Video and Audio streaming
- [x] Data channels
- [x] Simulcast
- [x] Scalable Video Coding ([SVC](https://www.w3.org/TR/webrtc-svc/))
- [x] Relay
- [x] Recording
- [x] WebRTC-HTTP Ingestion Protocol ([WHIP](https://www.ietf.org/archive/id/draft-ietf-wish-whip-09.html))
- [ ] WebRTC-HTTP Egress Protocol ([WHEP](https://www.ietf.org/archive/id/draft-murillo-whep-03.html))

## Who is this for

**Developers of WebRTC SFU servers**

This is an SDK, not a server application program. A key design principle was separating SFU-related logic from signaling protocols. So, you need a signaling logic to connect client browsers to your SFU server. It means, you can inject any logic in your signaling protocol.


## Contribution
Please check [GitHub repository](https://github.com/h3poteto/rheomesh). We welcome your contributions.

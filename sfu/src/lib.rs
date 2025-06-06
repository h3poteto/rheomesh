#![deny(missing_debug_implementations)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! # Rheomesh
//! Rheomesh is a WebRTC SFU library that provides a simple API for building real-time communication applications. This provides an SDK to help you build a WebRTC SFU server. Which means this doesn't provide signaling server, please create your own signaling server. And please use this for WebRTC SFU features.
//! [Here](https://github.com/h3poteto/rheomesh/blob/master/sfu/examples/media_server.rs) is an example SFU server for video streaming.
//!
//! ## Usage
//! Please refer the [official README](https://github.com/h3poteto/rheomesh/blob/master/sfu/README.md#usage).

/// Configuration for [`router::Router`], [`publish_transport::PublishTransport`] and [`subscribe_transport::SubscribeTransport`].
pub mod config;
/// DataChannel methods for publisher.
pub mod data_publisher;
/// DataChannel methods for subscriber.
pub mod data_subscriber;
pub mod error;
/// Track related methods for a published track.
pub mod local_track;
mod prober;
/// [`webrtc::peer_connection::RTCPeerConnection`] methods for publisher.
pub mod publish_transport;
/// Audio and video methods for track.
pub mod publisher;
pub mod recording_transport;
/// Relay is a module that provides methods to transfer publishers to other servers.
pub mod relay;
mod replay_channel;
/// Router is a module that determines which media to distribute to whom.
pub mod router;
/// RTP packet related module.
pub mod rtp;
/// [`webrtc::peer_connection::RTCPeerConnection`] methods for subscriber.
pub mod subscribe_transport;
/// Audio and video methods for subscriber.
pub mod subscriber;
pub mod track;
pub mod transport;
/// Worker is a module that manages multiple routers.
pub mod worker;

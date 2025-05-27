use std::{fmt, io};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    WebRTCError(#[from] webrtc::Error),
    #[error(transparent)]
    WebRTCUtilError(#[from] webrtc::util::Error),
    #[error(transparent)]
    SdpParseError(#[from] webrtc_sdp::error::SdpParserError),
    #[error(transparent)]
    SdpInternalError(#[from] webrtc_sdp::error::SdpParserInternalError),
    #[error(transparent)]
    IOError(#[from] io::Error),
    #[error(transparent)]
    BincodeDecodeError(#[from] bincode::error::DecodeError),
    #[error(transparent)]
    BincodeEncodeError(#[from] bincode::error::EncodeError),
    #[error(transparent)]
    TransportError(#[from] TransportError),
    #[error(transparent)]
    SubscriberError(#[from] SubscriberError),
    #[error(transparent)]
    PublisherError(#[from] PublisherError),
    #[error(transparent)]
    RtpParseError(#[from] RtpParseError),
    #[error(transparent)]
    RelayError(#[from] RelayError),
}

#[derive(thiserror::Error)]
#[error("{kind}: {message}")]
pub struct TransportError {
    pub kind: TransportErrorKind,
    pub message: String,
}

#[derive(thiserror::Error)]
#[error("{kind}: {message}")]
pub struct SubscriberError {
    pub kind: SubscriberErrorKind,
    pub message: String,
}

#[derive(thiserror::Error)]
#[error("{kind}: {message}")]
pub struct PublisherError {
    pub kind: PublisherErrorKind,
    pub message: String,
}

#[derive(thiserror::Error)]
#[error("{kind}: {message}")]
pub struct RtpParseError {
    pub kind: RtpParseErrorKind,
    pub message: String,
}

#[derive(thiserror::Error)]
#[error("{kind}: {message}")]
pub struct RelayError {
    pub kind: RelayErrorKind,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportErrorKind {
    #[error("peer connection error")]
    PeerConnectionError,
    #[error("local description error")]
    LocalDescriptionError,
    #[error("ice candidate error")]
    ICECandidateError,
    #[error("signaling state invalid error")]
    SignalingStateInvalidError,
    #[error("extmap parse error")]
    ExtmapParseError,
    #[error("ice restart error")]
    ICERestartError,
}

#[derive(Debug, thiserror::Error)]
pub enum SubscriberErrorKind {
    #[error("track not found error")]
    TrackNotFoundError,
    #[error("data channel not found error")]
    DataChannelNotFoundError,
}

#[derive(Debug, thiserror::Error)]
pub enum PublisherErrorKind {
    #[error("track not published error")]
    TrackNotPublishedError,
    #[error("data channel not published error")]
    DataChannelNotPublishedError,
    #[error("track not found error")]
    TrackNotFoundError,
}

#[derive(Debug, thiserror::Error)]
pub enum RtpParseErrorKind {
    #[error("AV1 dependency descriptor error")]
    AV1DependencyDescriptorError,
}

#[derive(Debug, thiserror::Error)]
pub enum RelayErrorKind {
    #[error("relay sender error")]
    RelaySenderError,
    #[error("relay receiver error")]
    RelayReceiverError,
}

impl Error {
    pub fn new_transport(message: String, kind: TransportErrorKind) -> Error {
        Error::TransportError(TransportError { kind, message })
    }

    pub fn new_subscriber(message: String, kind: SubscriberErrorKind) -> Error {
        Error::SubscriberError(SubscriberError { kind, message })
    }

    pub fn new_publisher(message: String, kind: PublisherErrorKind) -> Error {
        Error::PublisherError(PublisherError { kind, message })
    }

    pub fn new_rtp(message: String, kind: RtpParseErrorKind) -> Error {
        Error::RtpParseError(RtpParseError { kind, message })
    }

    pub fn new_relay(message: String, kind: RelayErrorKind) -> Error {
        Error::RelayError(RelayError { kind, message })
    }
}

impl fmt::Debug for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("rheomesh::TransportError");

        builder.field("kind", &self.kind);
        builder.field("message", &self.message);

        builder.finish()
    }
}

impl fmt::Debug for SubscriberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("rheomesh::SubscriberError");

        builder.field("kind", &self.kind);
        builder.field("message", &self.message);

        builder.finish()
    }
}

impl fmt::Debug for PublisherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("rheomesh::PublisherError");

        builder.field("kind", &self.kind);
        builder.field("message", &self.message);

        builder.finish()
    }
}

impl fmt::Debug for RtpParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("rheomesh::RtpParseError");

        builder.field("kind", &self.kind);
        builder.field("message", &self.message);

        builder.finish()
    }
}

impl fmt::Debug for RelayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("rheomesh::RelayError");

        builder.field("kind", &self.kind);
        builder.field("message", &self.message);

        builder.finish()
    }
}

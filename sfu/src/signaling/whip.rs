use std::sync::Arc;

use actix_web::{
    HttpRequest, HttpResponse,
    web::{self},
};
use async_trait::async_trait;
use webrtc::{
    ice_transport::ice_candidate::RTCIceCandidateInit,
    peer_connection::sdp::session_description::RTCSessionDescription,
};

use crate::{
    error::{Error, WhipSdpErrorKind},
    publish_transport::PublishTransport,
    transport::Transport,
};

use super::etag::ETagStore;

#[async_trait]
pub trait PublishTransportProvider: Send + Sync {
    async fn get_publish_transport(
        &self,
        session_id: &str,
    ) -> Result<Arc<PublishTransport>, actix_web::Error>;
}

#[derive(Debug, Clone)]
pub struct WhipEndpoint<P> {
    provider: P,
    etag_store: ETagStore,
}

impl<P> WhipEndpoint<P>
where
    P: PublishTransportProvider,
{
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            etag_store: ETagStore::new(),
        }
    }

    fn validate_request(&self, req: &HttpRequest, content_type: &str) -> Result<(), Error> {
        req.headers()
            .get("Content-Type")
            .and_then(|ct| ct.to_str().ok())
            .filter(|ct| *ct == content_type)
            .ok_or_else(|| {
                Error::new_whip_sdp(
                    "Invalid Content-Type header".to_string(),
                    WhipSdpErrorKind::InvalidContentTypeError,
                )
            })?;
        Ok(())
    }

    /// POST /whip/session_id - For SDP offer
    pub async fn handle_offer(
        &self,
        session_id: web::Path<String>,
        req: HttpRequest,
        body: web::Bytes,
    ) -> Result<HttpResponse, Error> {
        if let Err(e) = self.validate_request(&req, "application/sdp") {
            return Err(e);
        }

        let publish_transport = self
            .provider
            .get_publish_transport(&session_id)
            .await
            .map_err(|e| {
                Error::new_whip_sdp(
                    format!("Failed to get publish transport: {}", e),
                    WhipSdpErrorKind::InvalidSdpOfferError,
                )
            })?;

        //Parse SDP offer from body
        let sdp_string = String::from_utf8(body.to_vec()).map_err(|e| {
            Error::new_whip_sdp(e.to_string(), WhipSdpErrorKind::InvalidStringError)
        })?;
        let sdp_offer = RTCSessionDescription::offer(sdp_string)?;
        let sdp_answer = publish_transport.get_answer(sdp_offer).await?;

        let etag = self.etag_store.increment(&session_id).await;

        Ok(HttpResponse::Created()
            .content_type("application/sdp")
            .insert_header(("Location", format!("/whip/{}", session_id)))
            .insert_header(("ETag", etag))
            .body(sdp_answer.sdp))
    }

    /// PATCH /whip/session_id - For tricle ICE
    pub async fn handle_tricle_ice(
        &self,
        session_id: web::Path<String>,
        req: HttpRequest,
        body: web::Bytes,
    ) -> Result<HttpResponse, Error> {
        if let Err(e) = self.validate_request(&req, "application/trickle-ice-sdpfrag") {
            return Err(e);
        }

        self.etag_store.validate(&session_id, req).await?;

        let publish_transport = self
            .provider
            .get_publish_transport(&session_id)
            .await
            .map_err(|e| {
                Error::new_whip_sdp(
                    format!("Failed to get publish transport: {}", e),
                    WhipSdpErrorKind::InvalidSdpOfferError,
                )
            })?;

        let sdp_string = String::from_utf8(body.to_vec()).map_err(|e| {
            Error::new_whip_sdp(e.to_string(), WhipSdpErrorKind::InvalidStringError)
        })?;

        let candidate_init = RTCIceCandidateInit {
            candidate: sdp_string,
            sdp_mid: None,
            sdp_mline_index: None,
            username_fragment: None,
        };

        let _ = publish_transport.add_ice_candidate(candidate_init).await?;
        Ok(HttpResponse::NoContent().finish())
    }

    /// DELETE /whip/session_id - For ending the session
    pub async fn handle_delete(
        &self,
        session_id: web::Path<String>,
    ) -> Result<HttpResponse, Error> {
        self.etag_store.remove(&session_id).await;

        let publish_transport = self
            .provider
            .get_publish_transport(&session_id)
            .await
            .map_err(|e| {
                Error::new_whip_sdp(
                    format!("Failed to get publish transport: {}", e),
                    WhipSdpErrorKind::InvalidSdpOfferError,
                )
            })?;

        publish_transport.close().await?;

        Ok(HttpResponse::Ok().finish())
    }
}

impl<P> WhipEndpoint<P>
where
    P: PublishTransportProvider + Clone + 'static,
{
    pub fn configure(self, cfg: &mut web::ServiceConfig) {
        cfg.service(
            web::resource("/whip/{session_id}")
                .route(web::post().to(Self::handle_offer_route))
                .route(web::patch().to(Self::handle_tricle_ice_route))
                .route(web::delete().to(Self::handle_delete_route)),
        );
    }

    async fn handle_offer_route(
        endpoint: web::Data<Self>,
        session_id: web::Path<String>,
        req: HttpRequest,
        body: web::Bytes,
    ) -> Result<HttpResponse, actix_web::Error> {
        endpoint
            .handle_offer(session_id, req, body)
            .await
            .map_err(|e| e.into())
    }

    async fn handle_tricle_ice_route(
        endpoint: web::Data<Self>,
        session_id: web::Path<String>,
        req: HttpRequest,
        body: web::Bytes,
    ) -> Result<HttpResponse, actix_web::Error> {
        endpoint
            .handle_tricle_ice(session_id, req, body)
            .await
            .map_err(|e| e.into())
    }

    async fn handle_delete_route(
        endpoint: web::Data<Self>,
        session_id: web::Path<String>,
    ) -> Result<HttpResponse, actix_web::Error> {
        endpoint
            .handle_delete(session_id)
            .await
            .map_err(|e| e.into())
    }
}

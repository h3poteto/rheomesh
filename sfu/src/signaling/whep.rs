use std::sync::Arc;

use actix_web::{HttpRequest, HttpResponse, web};
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;
use webrtc::peer_connection::RTCSessionDescription;

use super::{etag::ETagStore, parser::parse_candidates};
use crate::{
    config::RID,
    error::{Error, WhepSdpErrorKind},
    router::{Router, RouterEvent},
    subscribe_transport::SubscribeTransport,
    track::Track,
    transport::Transport,
};

#[async_trait]
pub trait SubscribeTransportProvider: Send + Sync {
    async fn get_subscribe_transport(
        &self,
        session_id: &str,
    ) -> Result<Arc<SubscribeTransport>, actix_web::Error>;
}

#[derive(Debug, Clone)]
pub struct WhepEndpoint<P> {
    provider: P,
    etag_store: ETagStore,
}

impl<P> WhepEndpoint<P>
where
    P: SubscribeTransportProvider,
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
                Error::new_whep_sdp(
                    "Invalid Content-Type header".to_string(),
                    WhepSdpErrorKind::InvalidContentTypeError,
                )
            })?;
        Ok(())
    }

    async fn find_local_track(
        &self,
        router_event_sender: UnboundedSender<RouterEvent>,
        publisher_id: String,
    ) -> Result<Arc<dyn Track>, Error> {
        match Router::find_local_track(router_event_sender.clone(), publisher_id.clone(), RID::HIGH)
            .await
        {
            Ok(track) => Ok(track),
            Err(_) => {
                match Router::find_relayed_track(router_event_sender, publisher_id, RID::HIGH).await
                {
                    Ok(relayed_track) => Ok(relayed_track),
                    Err(err) => Err(err),
                }
            }
        }
    }

    /// POST /whep/session_id/publisher_id - For WHEP SDP offer
    async fn handle_offer(
        &self,
        params: web::Path<(String, String)>,
        req: HttpRequest,
        body: web::Bytes,
    ) -> Result<HttpResponse, Error> {
        if let Err(e) = self.validate_request(&req, "application/sdp") {
            return Err(e);
        }
        let session_id = params.0.clone();
        let publisher_id = params.1.clone();

        let subscribe_transport = self
            .provider
            .get_subscribe_transport(&session_id)
            .await
            .map_err(|e| {
                Error::new_whep_sdp(
                    format!("Failed to get subscribe transport: {}", e),
                    WhepSdpErrorKind::InvalidSdpOfferError,
                )
            })?;

        let track = self
            .find_local_track(
                subscribe_transport.router_event_sender.clone(),
                publisher_id.to_string(),
            )
            .await?;

        let _subscriber = subscribe_transport
            .subscribe_track(publisher_id.to_string(), track)
            .await?;

        // Parse SDP offer from body
        let sdp_string = String::from_utf8(body.to_vec()).map_err(|e| {
            Error::new_whep_sdp(e.to_string(), WhepSdpErrorKind::InvalidSdpOfferError)
        })?;
        let sdp_offer = RTCSessionDescription::offer(sdp_string)?;
        let answer = subscribe_transport.get_answer(sdp_offer).await?;

        let etag = self.etag_store.increment(&session_id).await;

        Ok(HttpResponse::Created()
            .content_type("application/sdp")
            .insert_header(("Location", format!("/whep/{}", session_id)))
            .insert_header(("ETag", etag))
            .body(answer.sdp))
    }

    /// PATCH /whep/session_id - For trickle ICE
    async fn handle_trickle_ice(
        &self,
        session_id: web::Path<String>,
        req: HttpRequest,
        body: web::Bytes,
    ) -> Result<HttpResponse, Error> {
        if let Err(e) = self.validate_request(&req, "application/trickle-ice-sdpfrag") {
            return Err(e);
        }

        let need_restart = self.etag_store.validate(&session_id, req).await?;

        let subscribe_transport = self
            .provider
            .get_subscribe_transport(&session_id)
            .await
            .map_err(|e| {
                Error::new_whep_sdp(
                    format!("Failed to get subscribe transport: {}", e),
                    WhepSdpErrorKind::InvalidSdpOfferError,
                )
            })?;

        let sdp_string = String::from_utf8(body.to_vec()).map_err(|e| {
            Error::new_whep_sdp(e.to_string(), WhepSdpErrorKind::InvalidSdpOfferError)
        })?;

        match need_restart {
            true => {
                // https://www.ietf.org/archive/id/draft-murillo-whep-03.html#section-4.1
                // RestartICE is an optional feature.
                // Since WHEP rarely requires restartICE, we do not support it.
                Ok(HttpResponse::MethodNotAllowed().finish())
            }
            false => {
                let candidates = parse_candidates(sdp_string.as_str());
                for candidate_init in candidates {
                    let _ = subscribe_transport
                        .add_ice_candidate(candidate_init)
                        .await?;
                }
                Ok(HttpResponse::NoContent().finish())
            }
        }
    }

    /// DELETE /whip/session_id - For ending the session
    async fn handle_delete(&self, session_id: web::Path<String>) -> Result<HttpResponse, Error> {
        self.etag_store.remove(&session_id).await;

        let subscribe_transport = self
            .provider
            .get_subscribe_transport(&session_id)
            .await
            .map_err(|e| {
                Error::new_whep_sdp(
                    format!("Failed to get subscribe transport: {}", e),
                    WhepSdpErrorKind::InvalidSdpOfferError,
                )
            })?;

        subscribe_transport.close().await?;

        Ok(HttpResponse::Ok().finish())
    }
}

impl<P> WhepEndpoint<P>
where
    P: SubscribeTransportProvider + Clone + 'static,
{
    pub fn configure(self, cfg: &mut web::ServiceConfig) {
        let endpoint = web::Data::new(self);

        cfg.service(
            web::resource("/whep/{session_id}/{publisher_id}")
                .route(web::post().to(Self::handle_offer_route)),
        )
        .service(
            web::resource("/whep/{session_id}")
                .route(web::patch().to(Self::handle_trickle_ice_route))
                .route(web::delete().to(Self::handle_delete_route)),
        )
        .app_data(endpoint);
    }

    async fn handle_offer_route(
        endpoint: web::Data<Self>,
        params: web::Path<(String, String)>,
        req: HttpRequest,
        body: web::Bytes,
    ) -> Result<HttpResponse, actix_web::Error> {
        endpoint
            .handle_offer(params, req, body)
            .await
            .map_err(|e| e.into())
    }

    async fn handle_trickle_ice_route(
        endpoint: web::Data<Self>,
        session_id: web::Path<String>,
        req: HttpRequest,
        body: web::Bytes,
    ) -> Result<HttpResponse, actix_web::Error> {
        endpoint
            .handle_trickle_ice(session_id, req, body)
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

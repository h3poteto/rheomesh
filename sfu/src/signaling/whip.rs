use std::sync::Arc;

use actix_web::{
    HttpRequest, HttpResponse,
    web::{self},
};
use async_trait::async_trait;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc_sdp::{attribute_type::SdpAttribute, parse_sdp};

use crate::{
    error::{Error, WhipSdpErrorKind},
    publish_transport::PublishTransport,
    transport::Transport,
};

use super::{
    etag::ETagStore,
    parser::{parse_candidates, parse_ice_pwd, parse_ice_ufrag},
    sdp_session::{get_current_ice, rfc8840},
};

/// A trait to provide [`PublishTransport`] instances based on session IDs. For example,
/// ```rust
/// struct SessionStore {
///   sessions: HashMap<String, Arc<PublishTransport>>,
/// }
/// #[async_trait::async_trait]
/// impl PublishTransportProvider for SessionStore {
///   async fn get_publish_transport(&self, session_id: &str) -> Result<Arc<PublishTransport>, actix_web::Error> {
///     let transport = self.sessions.get(session_id)
///       .cloned()
///       .ok_or_else(|| actix_web::error::ErrorNotFound("Session not found"))
///     Ok(transport)
///   }
/// }
/// ```
#[async_trait]
pub trait PublishTransportProvider: Send + Sync {
    ///Asynchronously retrieves a [`PublishTransport`] instance for the given session ID.
    async fn get_publish_transport(
        &self,
        session_id: &str,
    ) -> Result<Arc<PublishTransport>, actix_web::Error>;
}

/// WHIP signaling endpoint handler for [`actix_web`].
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
    async fn handle_offer(
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

    /// PATCH /whip/session_id - For trickle ICE
    async fn handle_tricle_ice(
        &self,
        session_id: web::Path<String>,
        req: HttpRequest,
        body: web::Bytes,
    ) -> Result<HttpResponse, Error> {
        if let Err(e) = self.validate_request(&req, "application/trickle-ice-sdpfrag") {
            return Err(e);
        }

        let need_restart = self.etag_store.validate(&session_id, req).await?;

        let publish_transport = self
            .provider
            .get_publish_transport(&session_id)
            .await
            .map_err(|e| {
                Error::new_whip_sdp(
                    format!("Failed to get publish transport: {}", e),
                    WhipSdpErrorKind::InvalidIceSdpfragError,
                )
            })?;

        let sdp_string = String::from_utf8(body.to_vec()).map_err(|e| {
            Error::new_whip_sdp(e.to_string(), WhipSdpErrorKind::InvalidStringError)
        })?;

        match need_restart {
            true => {
                let current_remote_sdp = publish_transport
                    .get_remote_description()
                    .await
                    .ok_or_else(|| {
                        Error::new_whip_sdp(
                            "Failed to get remote description".to_string(),
                            WhipSdpErrorKind::FailedToGetRemoteSdpError,
                        )
                    })?;
                let (current_ice_ufrag, current_ice_pwd) =
                    get_current_ice(&current_remote_sdp).await?;
                let ice_ufrag = parse_ice_ufrag(sdp_string.as_str());
                let ice_pwd = parse_ice_pwd(sdp_string.as_str());
                if ice_ufrag.is_some()
                    && ice_pwd.is_some()
                    && ice_ufrag.unwrap() != current_ice_ufrag
                    && ice_pwd.unwrap() != current_ice_pwd
                {
                    let mut current_parsed =
                        parse_sdp(&current_remote_sdp.sdp, false).map_err(|e| {
                            Error::new_whip_sdp(
                                format!("Failed to parse current remote SDP: {}", e),
                                WhipSdpErrorKind::FailedToGetRemoteSdpError,
                            )
                        })?;
                    let _ = current_parsed
                        .add_attribute(SdpAttribute::IceUfrag(ice_ufrag.unwrap().to_string()))?;
                    let _ = current_parsed
                        .add_attribute(SdpAttribute::IcePwd(ice_pwd.unwrap().to_string()))?;
                    let mut new_remote_sdp = current_remote_sdp.clone();
                    new_remote_sdp.sdp = current_parsed.to_string();
                    let answer = publish_transport.get_answer(new_remote_sdp).await?;

                    let candidates = parse_candidates(sdp_string.as_str());
                    for candidate_init in candidates {
                        let _ = publish_transport.add_ice_candidate(candidate_init).await?;
                    }

                    let return_sdp = rfc8840(&answer)?;

                    let etag = self.etag_store.increment(&session_id).await;
                    Ok(HttpResponse::Ok()
                        .content_type("application/trickle-ice-sdpfrag")
                        .insert_header(("ETag", etag))
                        .body(return_sdp))
                } else {
                    Err(Error::new_whip_sdp(
                        "ICE restart information missing or invalid".to_string(),
                        WhipSdpErrorKind::IceInformationMissingError,
                    ))
                }
            }
            false => {
                let candidates = parse_candidates(sdp_string.as_str());
                for candidate_init in candidates {
                    let _ = publish_transport.add_ice_candidate(candidate_init).await?;
                }
                Ok(HttpResponse::NoContent().finish())
            }
        }
    }

    /// DELETE /whip/session_id - For ending the session
    async fn handle_delete(&self, session_id: web::Path<String>) -> Result<HttpResponse, Error> {
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
    /// Configures the Actix web service with WHIP endpoint routes. This configures the following routes:
    ///- `POST /whip/{session_id}` for handling SDP offers.
    ///- `PATCH /whip/{session_id}` for handling ICE trickle updates.
    ///- `DELETE /whip/{session_id}` for ending the session.
    ///
    /// Please call this method within the `HttpServer` configuration.
    /// ```rust
    /// let endpoint = WhipEndpoint::new(provider);
    /// HttpServer::new(move || {
    ///   App::new()
    ///     .configure(|cfg| endpoint.clone().configure(cfg))
    /// })
    /// .bind("0.0.0.0:4000")?
    /// .run()
    /// .await
    /// ```
    pub fn configure(self, cfg: &mut web::ServiceConfig) {
        let endpoint = web::Data::new(self);

        cfg.service(
            web::resource("/whip/{session_id}")
                .route(web::post().to(Self::handle_offer_route))
                .route(web::patch().to(Self::handle_tricle_ice_route))
                .route(web::delete().to(Self::handle_delete_route)),
        )
        .app_data(endpoint);
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

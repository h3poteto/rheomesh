use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use actix_web::HttpRequest;
use tokio::sync::Mutex;

use crate::error::Error;

#[derive(Debug, Clone)]
pub struct ETagStore {
    counters: Arc<Mutex<HashMap<String, Arc<AtomicU64>>>>,
}

impl ETagStore {
    pub fn new() -> Self {
        ETagStore {
            counters: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn increment(&self, session_id: &str) -> String {
        let mut counters = self.counters.lock().await;
        let counter = counters
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)));
        let etag_value = counter.fetch_add(1, Ordering::SeqCst) + 1;
        format!("W/\"{}\"", etag_value)
    }

    pub async fn validate(&self, session_id: &str, req: HttpRequest) -> Result<(), Error> {
        let etag_header = req
            .headers()
            .get("If-Match")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| {
                Error::new_whip_sdp(
                    "Missing If-Match header".to_string(),
                    crate::error::WhipSdpErrorKind::MissingEtagError,
                )
            })?;

        let counters = self.counters.lock().await;
        if let Some(counter) = counters.get(session_id) {
            let current_value = counter.load(Ordering::SeqCst);
            let expected_etag = format!("W/\"{}\"", current_value);
            if etag_header == expected_etag {
                return Ok(());
            }
        }
        Err(Error::new_whip_sdp(
            "ETag mismatch".to_string(),
            crate::error::WhipSdpErrorKind::EtagMismatchError,
        ))
    }

    pub async fn remove(&self, session_id: &str) {
        let mut counters = self.counters.lock().await;
        counters.remove(session_id);
    }
}

use std::{collections::HashMap, sync::Arc};

use tokio::sync::{broadcast, Mutex};

use crate::{
    config::{MediaConfig, WorkerConfig},
    error::Error,
    relay::{receiver::RelayServer, sender::RelaySender},
    router::Router,
};

#[derive(Debug)]
pub struct Worker {
    pub routers: HashMap<String, Arc<Mutex<Router>>>,
    relay_sender: Arc<RelaySender>,
    stop_sender: broadcast::Sender<bool>,
}

impl Worker {
    pub async fn new(config: WorkerConfig) -> Result<Arc<Mutex<Self>>, Error> {
        let (stop_sender, _rx) = broadcast::channel(1);
        let relay_sender = RelaySender::new(
            config.relay_sender_port,
            config.relay_server_tcp_port,
            config.relay_server_udp_port,
        )
        .await?;

        let worker = Self {
            routers: HashMap::new(),
            relay_sender: Arc::new(relay_sender),
            stop_sender: stop_sender.clone(),
        };

        let worker = Arc::new(Mutex::new(worker));

        {
            let relay_server = RelayServer::new(
                config.relay_server_udp_port,
                config.relay_server_tcp_port,
                worker.clone(),
                stop_sender,
            )
            .await?;
            let relay_server = Arc::new(relay_server);

            {
                let relay_server = relay_server.clone();
                tokio::spawn(async move {
                    if let Err(err) = relay_server.run_tcp().await {
                        tracing::error!("Relay server TCP error: {}", err);
                    }
                });
            }

            tokio::spawn(async move {
                if let Err(err) = relay_server.run_udp().await {
                    tracing::error!("Relay server UDP error: {}", err);
                }
            });
        }

        Ok(worker)
    }

    pub fn new_router(&mut self, media_config: MediaConfig) -> Arc<Mutex<Router>> {
        let (router, id) = Router::new(media_config, self.relay_sender.clone());
        self.routers.insert(id, router.clone());
        router
    }

    pub fn close(&self) {
        let _ = self.stop_sender.send(true);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.stop_sender.send(true);
    }
}

use std::{collections::HashMap, sync::Arc};

use tokio::sync::{broadcast, mpsc, Mutex};

use crate::{
    config::{MediaConfig, WorkerConfig},
    error::Error,
    relay::{receiver::RelayServer, sender::RelaySender},
    router::Router,
};

/// Worker is responsible for managing routers and relay servers.
#[derive(Debug)]
pub struct Worker {
    pub routers: HashMap<String, Arc<Mutex<Router>>>,
    relay_sender: Arc<RelaySender>,
    stop_sender: broadcast::Sender<bool>,
    worker_event_sender: mpsc::UnboundedSender<WorkerEvent>,
}

impl Worker {
    /// Creates a new worker with the given configuration. And starts the relay server.
    pub async fn new(config: WorkerConfig) -> Result<Arc<Mutex<Self>>, Error> {
        let (stop_sender, _rx) = broadcast::channel(1);
        let relay_sender = RelaySender::new(config.relay_sender_port).await?;
        let (tx, rx) = mpsc::unbounded_channel::<WorkerEvent>();

        let worker = Self {
            routers: HashMap::new(),
            relay_sender: Arc::new(relay_sender),
            stop_sender: stop_sender.clone(),
            worker_event_sender: tx,
        };

        let worker = Arc::new(Mutex::new(worker));

        {
            let worker = worker.clone();
            tokio::spawn(async move {
                Self::worker_event_loop(worker, rx).await;
            });
        }

        {
            let relay_server =
                RelayServer::new(config.relay_server_tcp_port, worker.clone(), stop_sender).await?;
            let relay_server = Arc::new(relay_server);

            {
                let relay_server = relay_server.clone();
                tokio::spawn(async move {
                    if let Err(err) = relay_server.run_tcp().await {
                        tracing::error!("Relay server TCP error: {}", err);
                    }
                });
            }
        }

        Ok(worker)
    }

    /// Creates a new router and adds it to the worker.
    pub fn new_router(&mut self, media_config: MediaConfig) -> Arc<Mutex<Router>> {
        let (router, id) = Router::new(
            media_config,
            self.relay_sender.clone(),
            self.worker_event_sender.clone(),
        );
        self.routers.insert(id, router.clone());
        router
    }

    pub(crate) async fn worker_event_loop(
        worker: Arc<Mutex<Worker>>,
        mut event_receiver: mpsc::UnboundedReceiver<WorkerEvent>,
    ) {
        while let Some(event) = event_receiver.recv().await {
            match event {
                WorkerEvent::RouterRemoved(router_id) => {
                    let mut guard = worker.lock().await;
                    guard.routers.remove(&router_id);
                }
            }
        }
        tracing::debug!("Worker event loop finished");
    }

    /// Closes the worker and stops all routers.
    pub fn close(&self) {
        let _ = self.stop_sender.send(true);
    }
}

#[derive(Debug)]
pub(crate) enum WorkerEvent {
    RouterRemoved(String),
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.stop_sender.send(true);
    }
}

use std::{collections::VecDeque, sync::Arc};

use tokio::sync::{mpsc, Mutex};

#[derive(Debug)]
pub(crate) struct ReplayChannel<T> {
    history: Arc<Mutex<VecDeque<T>>>,
    sender: mpsc::Sender<T>,
    capacity: usize,
}

impl<T: Clone + Send + 'static> ReplayChannel<T> {
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<T>) {
        let (sender, receiver) = mpsc::channel(1024);
        let history = Arc::new(Mutex::new(VecDeque::with_capacity(capacity)));
        (
            Self {
                history,
                sender,
                capacity,
            },
            receiver,
        )
    }

    pub async fn send(&self, msg: T) {
        {
            let mut history = self.history.lock().await;
            if history.len() == self.capacity {
                history.pop_front();
            }
            history.push_back(msg.clone());
        }
        let _ = self.sender.send(msg).await;
    }

    pub async fn subscribe(&self) -> Vec<T> {
        let history = self.history.lock().await;
        history.iter().cloned().collect()
    }
}

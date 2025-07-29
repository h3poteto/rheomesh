use std::collections::HashMap;
use std::sync::Arc;

use actix::{Actor, Addr};
use tokio::sync::Mutex;

use rheomesh::{config::MediaConfig, router::Router, worker::Worker};

pub struct RoomOwner<T>
where
    T: Actor,
{
    rooms: HashMap<String, Arc<Room<T>>>,
    worker: Arc<Mutex<Worker>>,
}

impl<T> RoomOwner<T>
where
    T: Actor,
{
    pub fn new(worker: Arc<Mutex<Worker>>) -> Self {
        RoomOwner {
            rooms: HashMap::<String, Arc<Room<T>>>::new(),
            worker,
        }
    }

    pub fn find_by_id(&self, id: String) -> Option<Arc<Room<T>>> {
        self.rooms.get(&id).cloned()
    }

    pub async fn create_new_room(
        &mut self,
        id: String,
        config: MediaConfig,
    ) -> (Arc<Room<T>>, String) {
        let mut worker = self.worker.lock().await;
        let router = worker.new_router(config);
        #[allow(unused)]
        let mut router_id = "".to_string();
        {
            let locked = router.lock().await;
            router_id = locked.id.clone();
        }
        let room = Room::new(id.clone(), router);
        let a = Arc::new(room);
        self.rooms.insert(id.clone(), a.clone());
        (a, router_id)
    }

    pub fn remove_room(&mut self, room_id: String) {
        self.rooms.remove(&room_id);
    }
}

pub struct Room<T>
where
    T: Actor,
{
    pub id: String,
    pub router: Arc<Mutex<Router>>,
    users: std::sync::Mutex<Vec<Addr<T>>>,
}

impl<T> Room<T>
where
    T: Actor,
{
    pub fn new(id: String, router: Arc<Mutex<Router>>) -> Self {
        Self {
            id,
            router,
            users: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn add_user(&self, user: Addr<T>) {
        let mut users = self.users.lock().unwrap();
        users.push(user);
    }

    pub fn remove_user(&self, user: Addr<T>) -> usize {
        let mut users = self.users.lock().unwrap();
        users.retain(|u| u != &user);
        users.len()
    }

    pub fn get_peers(&self, user: &Addr<T>) -> Vec<Addr<T>> {
        let users = self.users.lock().unwrap();
        users.iter().filter(|u| u != &user).cloned().collect()
    }
}

use std::collections::HashMap;
use std::env;

use redis::Commands;

pub fn store_room(router_id: String, room_id: String, ip: String, port: u16) {
    // We are using redis to communicate between servers.
    // When you create your own server, you can use any communication method you like.
    // For example, WebSocket, gRPC, etc.
    let redis_host = env::var("REDIS_HOST").unwrap();
    let client = redis::Client::open(format!("redis://{}/", redis_host)).unwrap();
    let mut conn = client.get_connection().unwrap();
    let field = format!("{}|{}", ip, port);
    // hset overwrite the value if it exists.
    let _: () = conn.hset(room_id, field, router_id).unwrap();
}

pub fn delete_room(room_id: String, ip: String, port: u16) {
    let redis_host = env::var("REDIS_HOST").unwrap();
    let client = redis::Client::open(format!("redis://{}/", redis_host)).unwrap();
    let mut conn = client.get_connection().unwrap();
    let field = format!("{}|{}", ip, port);
    // hdel delete the value if it exists.
    let _: () = conn.hdel(room_id, field).unwrap();
}

pub fn get_pair_servers(
    room_id: String,
    my_ip: String,
    my_port: u16,
) -> HashMap<(String, u16), String> {
    let redis_host = env::var("REDIS_HOST").unwrap();
    let client = redis::Client::open(format!("redis://{}/", redis_host)).unwrap();
    let mut conn = client.get_connection().unwrap();
    let mut res: HashMap<String, String> = conn.hgetall(room_id).unwrap();
    let field = format!("{}|{}", my_ip, my_port);
    let _ = res.remove(&field);
    let parsed: HashMap<(String, u16), String> = res
        .into_iter()
        .filter_map(|(key, value)| {
            let parts: Vec<&str> = key.split('|').collect();
            if parts.len() == 2 {
                let ip = parts[0].to_string();
                let port = parts[1].parse::<u16>().unwrap_or(0);
                Some(((ip, port), value))
            } else {
                None
            }
        })
        .collect();
    parsed
}

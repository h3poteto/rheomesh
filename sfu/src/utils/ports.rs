use openport::{is_free_udp, pick_unused_port};

pub(crate) fn find_unused_port() -> Option<u16> {
    for _n in 0..100 {
        if let Some(port) = pick_unused_port(15000..65535) {
            if is_free_udp(port) {
                return Some(port);
            }
        }
    }
    None
}

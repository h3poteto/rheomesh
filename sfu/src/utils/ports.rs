use port_check::free_local_port_in_range;

pub(crate) fn find_unused_port() -> Option<u16> {
    let free_port = free_local_port_in_range(10000..=65535);
    free_port
}

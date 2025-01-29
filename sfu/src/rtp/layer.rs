#[derive(Debug, Clone)]
pub(crate) struct Layer {
    pub temporal_id: u8,
    pub spatial_id: u8,
}

impl Layer {
    pub fn new() -> Self {
        Self {
            temporal_id: 0,
            spatial_id: 0,
        }
    }
}

use bytes::{Bytes, BytesMut};

#[derive(Debug, Clone)]
pub struct Layer {
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

    pub fn marshal(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(2);
        buf.extend_from_slice(&[self.spatial_id]);
        buf.extend_from_slice(&[self.temporal_id]);
        buf.freeze()
    }

    pub fn unmarshal(bytes: &Bytes) -> Self {
        let spatial_id = bytes[0];
        let temporal_id = bytes[1];
        Self {
            spatial_id,
            temporal_id,
        }
    }
}

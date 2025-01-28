use std::collections::HashMap;

use bitvec::order::Msb0;
use bitvec::vec::BitVec;
use bytes::Bytes;

use crate::error::{self, Error};

#[derive(Debug, Clone)]
pub(crate) struct DependencyDescriptor {
    pub temporal_id: u8,
    pub spatial_id: u8,
}

#[derive(Debug, Clone)]
pub(crate) struct DependencyDescriptorParser {
    template_spatial_id: HashMap<u16, u32>,
    template_temporal_id: HashMap<u16, u32>,
    template_count: u16,
    template_id_offset: u16,
}

impl DependencyDescriptorParser {
    pub fn new() -> Self {
        Self {
            template_spatial_id: HashMap::<u16, u32>::new(),
            template_temporal_id: HashMap::<u16, u32>::new(),
            template_count: 0,
            template_id_offset: 0,
        }
    }

    pub fn parse(&mut self, payloads: &Bytes) -> Option<DependencyDescriptor> {
        let mut bits = BitIterator::new(payloads.clone());

        let (_, _, frame_dependency_template_id, _) = Self::mandatory_descriptor_fields(&mut bits);

        if payloads.len() > 3 {
            let template_dependency_structure_present_flag = bits.next(1).unwrap();
            let _active_decode_targets_present_flag = bits.next(1).unwrap();
            let _custom_dtis_flag = bits.next(1).unwrap();
            let _custom_fdiffs_flag = bits.next(1).unwrap();
            let _custom_chains_flag = bits.next(1).unwrap();

            if template_dependency_structure_present_flag > 0 {
                self.template_id_offset = bits.next(6).unwrap();
                let dt_cnt_minus_one = bits.next(5).unwrap();
                let _dt_cnt = dt_cnt_minus_one + 1;

                self.template_layers(&mut bits);
            }
        }

        match self.frame_dependency_definition(frame_dependency_template_id) {
            Ok((sid, tid)) => Some(DependencyDescriptor {
                temporal_id: tid.unwrap_or(0) as u8,
                spatial_id: sid.unwrap_or(0) as u8,
            }),
            Err(err) => {
                tracing::warn!("Failed to parse AV1: {}", err);
                None
            }
        }
    }

    fn mandatory_descriptor_fields(bits: &mut BitIterator) -> (u16, u16, u16, u16) {
        let start_of_frame = bits.next(1).unwrap();
        let end_of_frame = bits.next(1).unwrap();
        let frame_dependency_template_id = bits.next(6).unwrap();
        let frame_number = bits.next(16).unwrap();
        (
            start_of_frame,
            end_of_frame,
            frame_dependency_template_id,
            frame_number,
        )
    }

    fn template_layers(&mut self, bits: &mut BitIterator) {
        let mut temporal_id = 0;
        let mut spatial_id = 0;
        let mut template_count = 0;

        loop {
            self.template_spatial_id.insert(template_count, spatial_id);
            self.template_temporal_id
                .insert(template_count, temporal_id);
            template_count += 1;
            if let Some(next_layer_idc) = bits.next(2) {
                match next_layer_idc {
                    0 => {
                        // Same sid and tid
                    }
                    1 => {
                        temporal_id += 1;
                    }
                    2 => {
                        temporal_id = 0;
                        spatial_id += 1;
                    }
                    _ => continue,
                }
            } else {
                break;
            }
        }
        self.template_count = template_count;
    }

    fn frame_dependency_definition(
        &self,
        frame_dependency_template_id: u16,
    ) -> Result<(Option<u32>, Option<u32>), Error> {
        let template_index = (frame_dependency_template_id + 64 - self.template_id_offset) % 64;
        if template_index >= self.template_count {
            return Err(Error::new_rtp(
                "template_index is out of range".to_string(),
                error::RtpParseErrorKind::AV1DependencyDescriptorError,
            ));
        }
        let frame_spatial_id = self.template_spatial_id.get(&template_index).cloned();
        let frame_temporal_id = self.template_temporal_id.get(&template_index).cloned();
        Ok((frame_spatial_id, frame_temporal_id))
    }
}

struct BitIterator {
    bits: BitVec<u8, Msb0>,
    bit_index: usize,
}

impl BitIterator {
    fn new(bytes: Bytes) -> Self {
        let bits = BitVec::from_slice(bytes.as_ref());
        Self { bits, bit_index: 0 }
    }

    fn next(&mut self, n: u8) -> Option<u16> {
        let mut x = 0;
        for _ in 0..n {
            if let Some(next) = self.read_bit() {
                x = x * 2 + u16::from(next);
            } else {
                return None;
            }
        }
        Some(x)
    }

    fn read_bit(&mut self) -> Option<bool> {
        if self.bit_index >= self.bits.len() {
            return None;
        }

        let next = self.bits[self.bit_index];
        self.bit_index += 1;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bit_iterator() {
        let payloads = Bytes::from(vec![
            0b01000100, 0b00000010, 0b00011000, 0b00011011, 0b00000000, 0b00011000, 0b01001000,
        ]);
        let mut iter = BitIterator::new(payloads);
        assert_eq!(iter.next(1).unwrap(), 0b00000000);
        assert_eq!(iter.next(1).unwrap(), 0b00000001);
        assert_eq!(iter.next(6).unwrap(), 0b00000100);
        assert_eq!(iter.next(16).unwrap(), 0b00000010_00011000);
        assert_eq!(iter.next(1).unwrap(), 0b00000000);
        assert_eq!(iter.next(1).unwrap(), 0b00000000);
        assert_eq!(iter.next(1).unwrap(), 0b00000000);
        assert_eq!(iter.next(1).unwrap(), 0b00000001);
        assert_eq!(iter.next(1).unwrap(), 0b00000001);
        assert_eq!(iter.next(6).unwrap(), 0b00011000);
        assert_eq!(iter.next(5).unwrap(), 0b00000000);
        assert_eq!(iter.next(2).unwrap(), 0b00000000);
        assert_eq!(iter.next(2).unwrap(), 0b00000001);
        assert_eq!(iter.next(2).unwrap(), 0b00000010);
        assert_eq!(iter.next(2).unwrap(), 0b00000000);
        assert_eq!(iter.next(2).unwrap(), 0b00000001);
        assert_eq!(iter.next(2).unwrap(), 0b00000000);
        assert_eq!(iter.next(2).unwrap(), 0b00000010);
        assert_eq!(iter.next(2).unwrap(), 0b00000000);
        assert_eq!(iter.next(2), None);
    }
}

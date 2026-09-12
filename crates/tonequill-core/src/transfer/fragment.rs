use super::{Metadata, TransferError, metadata::data_packet_count, safe_filename};
use crate::{
    config::MAX_PAYLOAD_SIZE,
    protocol::packet::{FLAG_DATA, FLAG_METADATA, Packet},
};
use sha2::{Digest, Sha256};

/// Borrow the original bytes; allocate only the packet currently being emitted.
/// Metadata is first (sequence 0), data uses its own 0..count sequence space.
pub struct Fragmenter<'a> {
    metadata: Metadata,
    data: &'a [u8],
    next_frame: u32,
}

impl<'a> Fragmenter<'a> {
    pub fn new(transfer_id: u32, filename: &str, data: &'a [u8]) -> Result<Self, TransferError> {
        let data_packets = data_packet_count(data.len() as u64)?;
        Ok(Self {
            metadata: Metadata {
                transfer_id,
                file_size: data.len() as u64,
                data_packets,
                sha256: Sha256::digest(data).into(),
                filename: safe_filename(filename),
            },
            data,
            next_frame: 0,
        })
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

impl Iterator for Fragmenter<'_> {
    type Item = Packet;

    fn next(&mut self) -> Option<Packet> {
        if self.next_frame > self.metadata.data_packets {
            return None;
        }
        let mut packet = if self.next_frame == 0 {
            Packet::new(
                self.metadata.transfer_id,
                0,
                self.metadata.encode().expect("validated metadata"),
            )
        } else {
            let sequence = self.next_frame - 1;
            let start = sequence as usize * MAX_PAYLOAD_SIZE;
            let end = (start + MAX_PAYLOAD_SIZE).min(self.data.len());
            Packet::new(
                self.metadata.transfer_id,
                sequence as u16,
                self.data[start..end].to_vec(),
            )
        };
        packet.flags = if self.next_frame == 0 {
            FLAG_METADATA
        } else {
            FLAG_DATA
        };
        self.next_frame += 1;
        Some(packet)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.metadata.data_packets + 1 - self.next_frame) as usize;
        (remaining, Some(remaining))
    }
}
impl ExactSizeIterator for Fragmenter<'_> {}
impl std::iter::FusedIterator for Fragmenter<'_> {}

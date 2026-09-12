use super::{MAX_DATA_PACKETS, MAX_FILE_SIZE, Metadata, TransferError};
use crate::{
    config::MAX_PAYLOAD_SIZE,
    protocol::{
        framing::validate_version_flags,
        packet::{FLAG_DATA, FLAG_METADATA, Packet},
    },
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub struct ReassemblyLimits {
    pub max_transfers: usize,
    pub max_buffered_bytes: usize,
    pub max_buffered_packets: usize,
}
impl Default for ReassemblyLimits {
    fn default() -> Self {
        Self {
            max_transfers: 16,
            max_buffered_bytes: MAX_FILE_SIZE as usize,
            max_buffered_packets: MAX_DATA_PACKETS as usize,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Metadata,
    Data,
    Duplicate,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TransferProgress {
    pub received_packets: u32,
    pub expected_packets: Option<u32>,
    pub duplicate_frames: usize,
    pub buffered_bytes: usize,
    pub invalid: bool,
}

#[derive(Debug)]
pub struct VerifiedFile {
    pub metadata: Metadata,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
struct TransferState {
    metadata: Option<Metadata>,
    data: BTreeMap<u16, Vec<u8>>,
    duplicates: usize,
    bytes: usize,
    /// A conflict is permanent: later packets must not turn ambiguity into success.
    error: Option<TransferError>,
}

#[derive(Default)]
pub struct Reassembler {
    transfers: BTreeMap<u32, TransferState>,
    limits: ReassemblyLimits,
    buffered_bytes: usize,
    buffered_packets: usize,
}

impl Reassembler {
    pub fn new(limits: ReassemblyLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    pub fn transfer_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.transfers.keys().copied()
    }

    pub fn contains_data(&self, transfer_id: u32, sequence: u16) -> bool {
        self.transfers
            .get(&transfer_id)
            .is_some_and(|state| state.data.contains_key(&sequence))
    }

    /// The caller must pass packets obtained from CRC-valid framing. Constructed
    /// packets are useful for tests, but this method cannot authenticate their origin.
    /// Metadata may arrive after data. Storage is charged only for unique packets.
    pub fn push(&mut self, packet: Packet) -> Result<InsertOutcome, TransferError> {
        if !self.transfers.contains_key(&packet.transfer_id)
            && self.transfers.len() >= self.limits.max_transfers
        {
            return Err(TransferError::ResourceLimit("transfer IDs"));
        }
        let state = self.transfers.entry(packet.transfer_id).or_default();
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        let payload_len = packet.payload.len();
        let result = (|| {
            if validate_version_flags(packet.version, packet.flags).is_err()
                || payload_len > MAX_PAYLOAD_SIZE
            {
                return Err(TransferError::InvalidPacket(
                    "version, flags or payload length",
                ));
            }
            match packet.flags {
                FLAG_METADATA => {
                    if packet.sequence != 0 {
                        return Err(TransferError::InvalidPacket(
                            "metadata sequence must be zero",
                        ));
                    }
                    let metadata = Metadata::decode(packet.transfer_id, &packet.payload)?;
                    if let Some(previous) = &state.metadata {
                        if previous != &metadata {
                            return Err(TransferError::ConflictingMetadata);
                        }
                        state.duplicates += 1;
                        return Ok(InsertOutcome::Duplicate);
                    }
                    for (&sequence, payload) in &state.data {
                        if metadata.payload_size(sequence)? != payload.len() {
                            return Err(TransferError::SizeMismatch);
                        }
                    }
                    state.metadata = Some(metadata);
                    Ok(InsertOutcome::Metadata)
                }
                FLAG_DATA => {
                    if payload_len == 0 {
                        return Err(TransferError::InvalidPacket("empty data payload"));
                    }
                    if let Some(previous) = state.data.get(&packet.sequence) {
                        if previous != &packet.payload {
                            return Err(TransferError::ConflictingDuplicate(packet.sequence));
                        }
                        state.duplicates += 1;
                        return Ok(InsertOutcome::Duplicate);
                    }
                    if let Some(metadata) = &state.metadata
                        && metadata.payload_size(packet.sequence)? != payload_len
                    {
                        return Err(TransferError::SizeMismatch);
                    }
                    if payload_len
                        > self
                            .limits
                            .max_buffered_bytes
                            .saturating_sub(self.buffered_bytes)
                    {
                        return Err(TransferError::ResourceLimit("buffered bytes"));
                    }
                    if self.buffered_packets >= self.limits.max_buffered_packets {
                        return Err(TransferError::ResourceLimit("buffered packets"));
                    }
                    state.data.insert(packet.sequence, packet.payload);
                    state.bytes += payload_len;
                    Ok(InsertOutcome::Data)
                }
                _ => Err(TransferError::InvalidPacket(
                    "legacy payload is not a file-transfer frame",
                )),
            }
        })();
        match &result {
            Ok(InsertOutcome::Data) => {
                self.buffered_bytes += payload_len;
                self.buffered_packets += 1;
            }
            Err(error) => state.error = Some(error.clone()),
            _ => (),
        }
        result
    }

    pub fn progress(&self, transfer_id: u32) -> Result<TransferProgress, TransferError> {
        let state = self
            .transfers
            .get(&transfer_id)
            .ok_or(TransferError::UnknownTransfer(transfer_id))?;
        Ok(TransferProgress {
            received_packets: state.data.len() as u32,
            expected_packets: state.metadata.as_ref().map(|m| m.data_packets),
            duplicate_frames: state.duplicates,
            buffered_bytes: state.bytes,
            invalid: state.error.is_some(),
        })
    }

    pub fn missing_sequences(&self, transfer_id: u32) -> Result<Vec<u16>, TransferError> {
        let state = self
            .transfers
            .get(&transfer_id)
            .ok_or(TransferError::UnknownTransfer(transfer_id))?;
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        let metadata = state
            .metadata
            .as_ref()
            .ok_or(TransferError::MissingMetadata)?;
        Ok((0..metadata.data_packets)
            .filter(|&s| !state.data.contains_key(&(s as u16)))
            .map(|s| s as u16)
            .collect())
    }

    /// Invoke after the complete recording has been scanned, including later
    /// duplicates/conflicts. Completeness alone never bypasses size or SHA checks.
    pub fn finish(&self, transfer_id: u32) -> Result<VerifiedFile, TransferError> {
        let state = self
            .transfers
            .get(&transfer_id)
            .ok_or(TransferError::UnknownTransfer(transfer_id))?;
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        let metadata = state
            .metadata
            .as_ref()
            .ok_or(TransferError::MissingMetadata)?;
        let received = state.data.len() as u32;
        if received != metadata.data_packets {
            return Err(TransferError::MissingPackets {
                missing: metadata.data_packets - received,
                received,
                expected: metadata.data_packets,
            });
        }
        if state.bytes as u64 != metadata.file_size {
            return Err(TransferError::SizeMismatch);
        }
        let mut bytes = Vec::with_capacity(state.bytes);
        let mut hash = Sha256::new();
        for (&sequence, payload) in &state.data {
            if metadata.payload_size(sequence)? != payload.len() {
                return Err(TransferError::SizeMismatch);
            }
            bytes.extend_from_slice(payload);
            hash.update(payload);
        }
        if <[u8; 32]>::from(hash.finalize()) != metadata.sha256 {
            return Err(TransferError::HashMismatch);
        }
        Ok(VerifiedFile {
            metadata: metadata.clone(),
            bytes,
        })
    }
}

use super::{MAX_DATA_PACKETS, MAX_FILE_SIZE, TransferError};
use crate::config::MAX_PAYLOAD_SIZE;

const MAGIC: &[u8; 4] = b"SLTF";
pub const METADATA_VERSION: u8 = 1;
pub const METADATA_FIXED_SIZE: usize = 50;
pub const MAX_FILENAME_BYTES: usize = 96;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// Carried in the outer CRC-protected header, not duplicated in the payload.
    pub transfer_id: u32,
    pub file_size: u64,
    pub data_packets: u32,
    pub sha256: [u8; 32],
    /// Informational ASCII basename only. Never an output path.
    pub filename: String,
}

pub fn data_packet_count(size: u64) -> Result<u32, TransferError> {
    if size > MAX_FILE_SIZE {
        return Err(TransferError::FileTooLarge);
    }
    Ok(size.div_ceil(MAX_PAYLOAD_SIZE as u64) as u32)
}

fn valid_filename(name: &str) -> bool {
    if name.is_empty()
        || name.len() > MAX_FILENAME_BYTES
        || name.ends_with('.')
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

/// Convert a local filename to a bounded, portable display name. Remote names
/// are validated strictly instead of being interpreted as paths or normalized.
pub fn safe_filename(name: &str) -> String {
    let name: String = name
        .chars()
        .take(MAX_FILENAME_BYTES)
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim_end_matches('.');
    if valid_filename(name) {
        name.to_owned()
    } else {
        "file.bin".to_owned()
    }
}

impl Metadata {
    pub fn validate(&self) -> Result<(), TransferError> {
        if self.data_packets > MAX_DATA_PACKETS
            || self.data_packets != data_packet_count(self.file_size)?
        {
            return Err(TransferError::InvalidMetadata(
                "packet count does not match file size",
            ));
        }
        if !valid_filename(&self.filename) {
            return Err(TransferError::InvalidMetadata(
                "unsafe or overlong filename",
            ));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, TransferError> {
        self.validate()?;
        let mut bytes = Vec::with_capacity(METADATA_FIXED_SIZE + self.filename.len());
        bytes.extend_from_slice(MAGIC);
        bytes.push(METADATA_VERSION);
        bytes.extend_from_slice(&self.file_size.to_be_bytes());
        bytes.extend_from_slice(&self.data_packets.to_be_bytes());
        bytes.extend_from_slice(&self.sha256);
        bytes.push(self.filename.len() as u8);
        bytes.extend_from_slice(self.filename.as_bytes());
        Ok(bytes)
    }

    pub fn decode(transfer_id: u32, bytes: &[u8]) -> Result<Self, TransferError> {
        if bytes.len() < METADATA_FIXED_SIZE || &bytes[..4] != MAGIC || bytes[4] != METADATA_VERSION
        {
            return Err(TransferError::InvalidMetadata(
                "length, magic or schema version",
            ));
        }
        if bytes.len() != METADATA_FIXED_SIZE + bytes[49] as usize {
            return Err(TransferError::InvalidMetadata(
                "filename length or trailing bytes",
            ));
        }
        let metadata = Self {
            transfer_id,
            file_size: u64::from_be_bytes(bytes[5..13].try_into().unwrap()),
            data_packets: u32::from_be_bytes(bytes[13..17].try_into().unwrap()),
            sha256: bytes[17..49].try_into().unwrap(),
            filename: std::str::from_utf8(&bytes[50..])
                .map_err(|_| TransferError::InvalidMetadata("filename encoding"))?
                .to_owned(),
        };
        metadata.validate()?;
        Ok(metadata)
    }

    pub fn payload_size(&self, sequence: u16) -> Result<usize, TransferError> {
        if u32::from(sequence) >= self.data_packets {
            return Err(TransferError::InvalidPacket(
                "sequence outside declared transfer",
            ));
        }
        let remaining = self
            .file_size
            .checked_sub(u64::from(sequence) * MAX_PAYLOAD_SIZE as u64)
            .filter(|&n| n > 0)
            .ok_or(TransferError::SizeMismatch)?;
        Ok(remaining.min(MAX_PAYLOAD_SIZE as u64) as usize)
    }
}

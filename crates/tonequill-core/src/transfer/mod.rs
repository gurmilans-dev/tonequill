//! File transfer over CRC-validated V0 packets, without filesystem access.
//! Fragmentation and reassembly are independent of DSP; waveform bridges to the PHY.
pub mod fragment;
pub mod metadata;
pub mod reassembly;
pub mod reliable;
pub mod waveform;

pub use fragment::Fragmenter;
pub use metadata::{Metadata, safe_filename};
pub use reassembly::{
    InsertOutcome, Reassembler, ReassemblyLimits, TransferProgress, VerifiedFile,
};

use thiserror::Error;

pub const MAX_DATA_PACKETS: u32 = u16::MAX as u32 + 1;
pub const MAX_FILE_SIZE: u64 = MAX_DATA_PACKETS as u64 * crate::config::MAX_PAYLOAD_SIZE as u64;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum TransferError {
    #[error("file exceeds transfer limit of {MAX_FILE_SIZE} bytes (65536 data packets)")]
    FileTooLarge,
    #[error("invalid transfer metadata: {0}")]
    InvalidMetadata(&'static str),
    #[error("invalid transfer packet: {0}")]
    InvalidPacket(&'static str),
    #[error("conflicting metadata for the same transfer ID")]
    ConflictingMetadata,
    #[error("conflicting duplicate data packet at sequence {0}")]
    ConflictingDuplicate(u16),
    #[error("metadata is missing")]
    MissingMetadata,
    #[error("missing {missing} data packets (received {received}, expected {expected})")]
    MissingPackets {
        missing: u32,
        received: u32,
        expected: u32,
    },
    #[error("reconstructed file size does not match metadata")]
    SizeMismatch,
    #[error("final SHA-256 mismatch")]
    HashMismatch,
    #[error("reassembly resource limit reached: {0}")]
    ResourceLimit(&'static str),
    #[error("unknown transfer ID {0:08X}")]
    UnknownTransfer(u32),
}

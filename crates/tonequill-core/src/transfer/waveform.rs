//! Independently framed packets with bounded silence. The existing BFSK modulator
//! is used unchanged for each packet; callers can write one frame at a time.
use super::{Metadata, TransferError};
use crate::{
    config::SAMPLE_RATE,
    modulation::bfsk::{modulate, samples_per_symbol},
    protocol::{
        bitstream::bytes_to_bits,
        framing::{FrameError, PREFIX_SIZE, encode_frame},
        packet::Packet,
    },
};

pub const INTER_FRAME_SAMPLES: usize = SAMPLE_RATE as usize / 5; // 200 ms = 20 symbols
pub const EDGE_PADDING_SAMPLES: usize = SAMPLE_RATE as usize / 5;

pub fn frame_samples(packet: &Packet) -> Result<Vec<f32>, FrameError> {
    Ok(modulate(&bytes_to_bits(&encode_frame(packet)?)))
}

#[derive(Debug, PartialEq, Eq)]
pub struct TransmissionPlan {
    pub data_packets: u32,
    pub control_frames: u32,
    pub total_frames: u32,
    pub frame_bytes: u64,
    pub modulated_bits: u64,
    pub silence_samples: u64,
    pub total_samples: u64,
}
impl TransmissionPlan {
    pub fn new(metadata: &Metadata) -> Result<Self, TransferError> {
        let metadata_bytes = metadata.encode()?.len() as u64;
        let total_frames = metadata.data_packets + 1;
        let frame_bytes = metadata.file_size
            + metadata_bytes
            + u64::from(total_frames) * (PREFIX_SIZE + 4) as u64;
        let modulated_bits = frame_bytes * 8;
        let silence_samples = 2 * EDGE_PADDING_SAMPLES as u64
            + u64::from(total_frames - 1) * INTER_FRAME_SAMPLES as u64;
        Ok(Self {
            data_packets: metadata.data_packets,
            control_frames: 1,
            total_frames,
            frame_bytes,
            modulated_bits,
            silence_samples,
            total_samples: modulated_bits * samples_per_symbol() as u64 + silence_samples,
        })
    }
    pub fn duration_seconds(&self) -> f64 {
        self.total_samples as f64 / SAMPLE_RATE as f64
    }
}

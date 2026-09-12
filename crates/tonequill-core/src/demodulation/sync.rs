//! Compatibility entry point for callers needing only an acquisition summary.
//! Packet decoding should use receiver::receive / receiver::inspect so estimated
//! timing and calibration are applied throughout the frame.
use crate::{
    demodulation::acquisition::acquire, modulation::bfsk::samples_per_symbol,
    signal::tones::ToneTrack,
};

#[derive(Debug, Clone, Copy)]
pub struct SyncResult {
    pub start_sample: usize,
    pub matched_bits: usize,
    pub tested_bits: usize,
    pub quality: f32,
}

/// Searches the whole recording using all 64 preamble bits and the sync word.
/// Returns the first acquisition; this alone does not imply a CRC-valid packet.
pub fn find_frame_start(samples: &[f32]) -> Option<SyncResult> {
    if samples.iter().any(|s| !s.is_finite()) {
        return None;
    }
    let full = ToneTrack::new(samples, samples_per_symbol());
    let half = ToneTrack::new(samples, samples_per_symbol() / 2);
    let candidate = acquire(&full, &half, samples.len(), true, true, true, true)
        .into_iter()
        .next()?;
    Some(SyncResult {
        start_sample: candidate.start_sample.round() as usize,
        matched_bits: candidate.preamble_matches,
        tested_bits: 64,
        quality: candidate.quality as f32,
    })
}

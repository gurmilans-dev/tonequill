//! Bounded offline scanning around the unchanged single-window PHY. A 24-second
//! ownership region has one second of left context and 24 seconds of lookahead.
//! Every possible 280-byte frame (22.4 s, including +/-5000 ppm clock drift) fits.
//! Failed headers never determine the search advance; later candidates survive.
use super::{FrameAttempt, ReceiveError, ReceiverOptions, inspect};
use crate::config::SAMPLE_RATE;

pub const SCAN_STRIDE_SAMPLES: usize = 24 * SAMPLE_RATE as usize;
const CONTEXT: usize = SAMPLE_RATE as usize;
const LOOKAHEAD: usize = 24 * SAMPLE_RATE as usize;
const CAPACITY: usize = CONTEXT + SCAN_STRIDE_SAMPLES + LOOKAHEAD;

pub struct FrameScanner {
    options: ReceiverOptions,
    buffer: Vec<f32>,
    offset: u64,
    owned_start: u64,
    valid_until: f64,
    last_candidate: Option<f64>,
}
impl Default for FrameScanner {
    fn default() -> Self {
        Self::new(ReceiverOptions::default())
    }
}
impl FrameScanner {
    pub fn new(options: ReceiverOptions) -> Self {
        Self {
            options,
            buffer: Vec::new(),
            offset: 0,
            owned_start: 0,
            valid_until: 0.0,
            last_candidate: None,
        }
    }

    /// Input can be split at any sample. Retained audio and tone tracks are bounded
    /// independently of recording length. Consume returned attempts between pushes.
    pub fn push(&mut self, mut samples: &[f32]) -> Result<Vec<FrameAttempt>, ReceiveError> {
        if samples.iter().any(|s| !s.is_finite()) {
            return Err(ReceiveError::InvalidSamples);
        }
        let mut attempts = Vec::new();
        while !samples.is_empty() {
            let count = samples.len().min(CAPACITY - self.buffer.len());
            self.buffer.extend_from_slice(&samples[..count]);
            samples = &samples[count..];
            if self.buffer.len() == CAPACITY {
                attempts.extend(self.process(false)?);
            }
        }
        Ok(attempts)
    }

    pub fn finish(mut self) -> Result<Vec<FrameAttempt>, ReceiveError> {
        self.process(true)
    }

    fn process(&mut self, final_window: bool) -> Result<Vec<FrameAttempt>, ReceiveError> {
        let owned_end = if final_window {
            u64::MAX
        } else {
            self.owned_start + SCAN_STRIDE_SAMPLES as u64
        };
        let attempts = match inspect(&self.buffer, self.options) {
            Ok(attempts) => attempts,
            Err(ReceiveError::AcquisitionFailed) => Vec::new(),
            Err(error) => return Err(error),
        };
        let mut selected = Vec::new();
        for mut attempt in attempts {
            let start = attempt.acquisition.start_sample + self.offset as f64;
            if start < self.owned_start as f64
                || start >= owned_end as f64
                || start < self.valid_until
                || self
                    .last_candidate
                    .is_some_and(|last| (start - last).abs() < 480.0)
            {
                continue;
            }
            attempt.acquisition.start_sample = start;
            for symbol in &mut attempt.symbols {
                symbol.center_sample += self.offset as f64;
            }
            if attempt.result.is_ok() {
                // Suppress nested preamble-like payloads only inside validated
                // symbols. Leave half a symbol of margin for the next real frame.
                self.valid_until = attempt.symbols.last().map_or(start, |s| s.center_sample);
            }
            self.last_candidate = Some(start);
            selected.push(attempt);
        }
        if !final_window {
            self.owned_start = owned_end;
            let next_offset = self.owned_start - CONTEXT as u64;
            self.buffer.drain(..(next_offset - self.offset) as usize);
            self.offset = next_offset;
        }
        Ok(selected)
    }
}

/// Convenience for existing in-memory callers. Unlike receive(), this returns
/// every independent CRC-valid packet plus failed-candidate diagnostics.
pub fn scan(samples: &[f32], options: ReceiverOptions) -> Result<Vec<FrameAttempt>, ReceiveError> {
    let mut scanner = FrameScanner::new(options);
    let mut attempts = scanner.push(samples)?;
    attempts.extend(scanner.finish()?);
    Ok(attempts)
}

//! Incremental adapter for the existing PHY. Inspect at most every 250 ms and
//! release complete frames promptly; incomplete frames retain their samples.
//! No acquisition, calibration, clock, timing or symbol thresholds are changed.
use super::{FrameAttempt, ReceiveError, ReceiverOptions, inspect_with_search_limit};
use crate::{config::SAMPLE_RATE, demodulation::acquisition::COARSE_STEP};

const POLL: usize = SAMPLE_RATE as usize / 4;
const CONTEXT: usize = SAMPLE_RATE as usize;
const CAPACITY: usize = 26 * SAMPLE_RATE as usize;

pub struct LiveDecoder {
    buffer: Vec<f32>,
    offset: u64,
    consumed_until: f64,
    uninspected: usize,
    options: ReceiverOptions,
    pending_start: Option<f64>,
}
impl Default for LiveDecoder {
    fn default() -> Self {
        Self::new(ReceiverOptions::default())
    }
}
impl LiveDecoder {
    pub fn new(options: ReceiverOptions) -> Self {
        Self {
            buffer: Vec::new(),
            offset: 0,
            consumed_until: 0.0,
            uninspected: 0,
            options,
            pending_start: None,
        }
    }

    /// Discard partial audio on a device overrun or while transmitting locally.
    /// Higher-level reassembly remains intact and ARQ repairs missing packets.
    pub fn reset(&mut self) {
        self.offset += self.buffer.len() as u64;
        self.buffer.clear();
        self.consumed_until = self.offset as f64;
        self.uninspected = 0;
        self.pending_start = None;
    }

    pub fn push(&mut self, mut samples: &[f32]) -> Result<Vec<FrameAttempt>, ReceiveError> {
        if samples.iter().any(|s| !s.is_finite()) {
            return Err(ReceiveError::InvalidSamples);
        }
        let mut attempts = Vec::new();
        while !samples.is_empty() {
            let take = samples.len().min(POLL - self.uninspected);
            self.buffer.extend_from_slice(&samples[..take]);
            self.uninspected += take;
            samples = &samples[take..];
            if self.uninspected == POLL {
                attempts.extend(self.process()?);
                self.uninspected = 0;
            }
        }
        Ok(attempts)
    }

    pub fn flush(&mut self) -> Result<Vec<FrameAttempt>, ReceiveError> {
        self.uninspected = 0;
        self.process()
    }

    fn process(&mut self) -> Result<Vec<FrameAttempt>, ReceiveError> {
        let inspect =
            |last_start| match inspect_with_search_limit(&self.buffer, self.options, last_start) {
                Ok(attempts) => Ok(attempts),
                Err(ReceiveError::AcquisitionFailed) => Ok(Vec::new()),
                Err(error) => Err(error),
            };
        let pending = self.pending_start.take();
        let last_start = pending.map_or(usize::MAX, |start| {
            ((start - self.offset as f64).max(0.0) as usize).saturating_add(480)
        });
        let mut attempts = inspect(last_start)?;
        // A pending frame already suppresses nested candidates. Search only
        // around its known onset while its bytes arrive, avoiding a quadratic
        // preamble search over the growing payload at every 250 ms poll.
        if pending.is_some()
            && !attempts
                .iter()
                .find(|a| a.acquisition.start_sample + self.offset as f64 >= self.consumed_until)
                .is_some_and(|a| matches!(a.result, Err(ReceiveError::Truncated { .. })))
        {
            // Once it resolves, search the complete retained audio before any
            // trim so a corrupt long length cannot hide later complete frames.
            attempts = inspect(usize::MAX)?;
        }
        let mut keep_from = self.buffer.len().saturating_sub(CONTEXT);
        let mut selected = Vec::new();
        for mut attempt in attempts {
            let start = attempt.acquisition.start_sample + self.offset as f64;
            if start < self.consumed_until {
                continue;
            }
            if matches!(attempt.result, Err(ReceiveError::Truncated { .. })) {
                self.pending_start = Some(start);
                // Do not expose preamble-like bytes nested inside a partial frame.
                // A corrupt plausible length can defer later frames by at most
                // the maximum PHY frame duration, included in ARQ timeout budgets.
                keep_from = keep_from
                    .min((attempt.acquisition.start_sample.max(0.0) as usize).saturating_sub(480));
                break;
            }
            attempt.acquisition.start_sample = start;
            for symbol in &mut attempt.symbols {
                symbol.center_sample += self.offset as f64;
            }
            self.consumed_until = if attempt.result.is_ok() {
                attempt.symbols.last().map_or(start, |s| s.center_sample)
            } else {
                // Never use an unvalidated length to advance past a candidate.
                start + 64.0 * 480.0
            };
            selected.push(attempt);
        }
        // A valid maximum frame, including supported clock/timing displacement,
        // fits within 26 s. The cap also bounds pathological incomplete candidates.
        // Keep the acquisition grid fixed in absolute sample coordinates.
        // Trimming at a refined onset otherwise shifts the coarse search and
        // can select a different acquisition path for the very same frame.
        keep_from -= keep_from % COARSE_STEP;
        let capacity_floor = self.buffer.len().saturating_sub(CAPACITY);
        keep_from = keep_from.max(capacity_floor.div_ceil(COARSE_STEP) * COARSE_STEP);
        if keep_from > 0 {
            self.buffer.drain(..keep_from);
            self.offset += keep_from as u64;
        }
        Ok(selected)
    }
}

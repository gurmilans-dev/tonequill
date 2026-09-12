//! Offline V0 receiver. Acquisition and soft-symbol diagnostics are retained on
//! failure, but a Packet is exposed only after exact framing and CRC validation.
use crate::{
    demodulation::acquisition::{self, Acquisition, MAX_CLOCK_PPM},
    modulation::bfsk::samples_per_symbol,
    protocol::{
        bitstream::bits_to_bytes,
        framing::{self, FrameError, MAX_FRAME_SIZE, PREFIX_SIZE},
        packet::Packet,
    },
    signal::tones::{HOP, ToneTrack},
};
use thiserror::Error;

mod equalizer;
mod live;
pub use equalizer::EqualizerMetrics;
mod stream;
pub use live::LiveDecoder;
pub use stream::{FrameScanner, SCAN_STRIDE_SAMPLES, scan};

#[derive(Debug, Clone, Copy)]
pub struct ReceiverOptions {
    pub calibrate_carriers: bool,
    pub estimate_clock: bool,
    pub track_timing: bool,
    /// Full-symbol decisions are retained as an experimental A/B control.
    pub full_symbol_decisions: bool,
    /// Track startup changes in relative carrier gain using known preamble blocks.
    pub train_decision_threshold: bool,
    /// Retry failed energy decisions with a training-selected complex channel model.
    pub equalize: bool,
}
impl Default for ReceiverOptions {
    fn default() -> Self {
        Self {
            calibrate_carriers: true,
            estimate_clock: true,
            track_timing: true,
            full_symbol_decisions: false,
            train_decision_threshold: true,
            equalize: true,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReceiveError {
    #[error("audio contains non-finite samples")]
    InvalidSamples,
    #[error("no Tonequill preamble and sync word acquired")]
    AcquisitionFailed,
    #[error("truncated frame: expected {expected_bits} bits, received {received_bits}")]
    Truncated {
        expected_bits: usize,
        received_bits: usize,
    },
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[derive(Debug, Clone)]
pub struct SymbolMetrics {
    pub index: usize,
    pub center_sample: f64,
    pub bit: bool,
    pub carrier_power: [f64; 2],
    /// Signed decision contrast in [-1, 1]: energy difference, or the complex
    /// model's residual-cost contrast when FrameAttempt.equalizer is present.
    pub soft_value: f64,
    pub confidence: f64,
    /// E1/E0 energy threshold (diagnostic only on the complex equalizer path).
    pub decision_ratio: f64,
    /// Phase correction applied at the preceding transition, in input samples.
    pub timing_correction: f64,
}

#[derive(Debug)]
pub struct FrameAttempt {
    pub acquisition: Acquisition,
    pub symbols: Vec<SymbolMetrics>,
    pub final_samples_per_symbol: f64,
    pub result: Result<Packet, ReceiveError>,
    pub equalizer: Option<EqualizerMetrics>,
}
impl FrameAttempt {
    pub fn bits(&self) -> Vec<bool> {
        self.symbols.iter().map(|s| s.bit).collect()
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct BitComparison {
    pub reference_bits: usize,
    pub compared_bits: usize,
    pub bit_errors: usize,
    pub missing_bits: usize,
}
impl BitComparison {
    /// BER is conditional on observed bits; missing bits are reported separately.
    pub fn ber(&self) -> Option<f64> {
        (self.compared_bits > 0).then(|| self.bit_errors as f64 / self.compared_bits as f64)
    }
}
pub fn compare_bits(reference: &[bool], received: &[bool]) -> BitComparison {
    let compared = reference.len().min(received.len());
    BitComparison {
        reference_bits: reference.len(),
        compared_bits: compared,
        bit_errors: reference
            .iter()
            .zip(received)
            .filter(|(a, b)| a != b)
            .count(),
        missing_bits: reference.len() - compared,
    }
}

/// Search the complete recording, including after failed frames. Input is finite,
/// mono, 48 kHz PCM in floating point. Padding may contain arbitrary noise.
pub fn inspect(
    samples: &[f32],
    options: ReceiverOptions,
) -> Result<Vec<FrameAttempt>, ReceiveError> {
    inspect_with_search_limit(samples, options, usize::MAX)
}

fn central_track(samples: &[f32], options: ReceiverOptions) -> ToneTrack {
    // Share the observation grid with the diagnostic energy probe so its
    // symbols, timing corrections and confidence describe the actual receiver.
    ToneTrack::with_hop(
        samples,
        samples_per_symbol() / 2,
        if options.equalize { HOP / 2 } else { HOP },
    )
}

fn inspect_with_search_limit(
    samples: &[f32],
    options: ReceiverOptions,
    last_start: usize,
) -> Result<Vec<FrameAttempt>, ReceiveError> {
    if samples.iter().any(|x| !x.is_finite()) {
        return Err(ReceiveError::InvalidSamples);
    }
    let full = ToneTrack::new(samples, samples_per_symbol());
    // Dense central observations keep narrow acquisition peaks visible for
    // arbitrary sample alignments; the original no-equalizer control is unchanged.
    let half = central_track(samples, options);
    let candidates = acquisition::acquire(
        &full,
        &half,
        samples
            .len()
            .min(last_start.saturating_add(80 * samples_per_symbol())),
        options.calibrate_carriers,
        options.estimate_clock,
        options.train_decision_threshold,
        options.equalize,
    );
    if candidates.is_empty() {
        return Err(ReceiveError::AcquisitionFailed);
    }
    Ok(candidates
        .into_iter()
        .map(|acquisition| {
            let attempt = decode_candidate(
                &full,
                if options.full_symbol_decisions {
                    &full
                } else {
                    &half
                },
                samples.len(),
                acquisition.clone(),
                options,
                None,
            );
            if options.equalize && attempt.result.is_err() {
                let mut incomplete = None;
                for model in equalizer::Equalizer::train(&half, &acquisition) {
                    let retry = equalizer::decode(&half, acquisition.clone(), model);
                    if retry.result.is_ok() {
                        return retry;
                    }
                    // A complete hypothesis can succeed even when an earlier
                    // one predicts a longer frame. Try the whole bounded list
                    // before retaining the best incomplete candidate.
                    if incomplete.is_none()
                        && matches!(retry.result, Err(ReceiveError::Truncated { .. }))
                    {
                        incomplete = Some(retry);
                    }
                }
                if let Some(incomplete) = incomplete {
                    return incomplete;
                }
            }
            attempt
        })
        .collect())
}

/// Return the first CRC-valid packet in a longer recording. No payload is
/// returned if all candidates fail. Use inspect() for failure diagnostics.
pub fn receive(samples: &[f32]) -> Result<Packet, ReceiveError> {
    let attempts = inspect(samples, ReceiverOptions::default())?;
    let mut last_error = ReceiveError::AcquisitionFailed;
    for attempt in attempts {
        match attempt.result {
            Ok(packet) => return Ok(packet),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn transition_error(
    full: &ToneTrack,
    boundary: f64,
    period: f64,
    gain: [f64; 2],
    next_bit: bool,
) -> Option<f64> {
    let direction = if next_bit { 1.0 } else { -1.0 };
    let radius = 0.22 * period;
    let mut previous: Option<(f64, f64)> = None;
    let mut best: Option<f64> = None;
    for step in 0..=((2.0 * radius / HOP as f64).ceil() as usize) {
        let position = boundary - radius + step as f64 * HOP as f64;
        let obs = full.at(position)?;
        let value = acquisition::soft(obs.power, gain) * direction;
        if let Some((last_pos, last_value)) = previous
            && last_value <= 0.0
            && value > 0.0
        {
            let cross = last_pos + (position - last_pos) * (-last_value) / (value - last_value);
            let error: f64 = cross - boundary;
            if best.is_none_or(|b| error.abs() < b.abs()) {
                best = Some(error);
            }
        }
        previous = Some((position, value));
    }
    best
}

fn decode_candidate(
    full: &ToneTrack,
    decisions: &ToneTrack,
    sample_count: usize,
    acquisition: Acquisition,
    options: ReceiverOptions,
    diagnostic_bit_count: Option<usize>,
) -> FrameAttempt {
    let gain = if options.calibrate_carriers {
        acquisition.carrier_power
    } else {
        [1.0; 2]
    };
    // Classification learns a noise/transition-aware boundary; timing still uses
    // physical carrier powers to avoid moving edges to a classifier's threshold.
    let nominal = samples_per_symbol() as f64;
    let mut period = acquisition.samples_per_symbol;
    let mut boundary = acquisition.start_sample;
    let mut symbols: Vec<SymbolMetrics> = Vec::new();
    let mut expected = diagnostic_bit_count
        .unwrap_or(PREFIX_SIZE * 8)
        .min(MAX_FRAME_SIZE * 8);
    let mut frame_error = None;
    while symbols.len() < expected && symbols.len() < MAX_FRAME_SIZE * 8 {
        let index = symbols.len();
        let decision_gain = [1.0, acquisition.decision_ratio_at(index)];
        // Enough samples must remain for the complete decision window. The
        // unused guard at the symbol end may be absent; CRC still validates all bits.
        if boundary + period / 2.0 > sample_count as f64 {
            break;
        }
        let Some(mut obs) = decisions.at(boundary + period / 2.0) else {
            break;
        };
        let mut soft = acquisition::soft(obs.power, decision_gain);
        let mut correction = 0.0;
        if options.track_timing
            && let Some(previous) = symbols.last()
            && previous.bit != (soft > 0.0)
            && previous.confidence > 0.6
            && soft.abs() > 0.6
            && let Some(error) = transition_error(full, boundary, period, gain, soft > 0.0)
        {
            // A gently damped second-order loop; only confident transitions steer
            // it. During constant-bit runs the preamble clock estimate free-runs.
            correction = 0.12 * error;
            boundary += correction;
            period = (period + 0.0004 * error).clamp(
                nominal * (1.0 - MAX_CLOCK_PPM * 1e-6),
                nominal * (1.0 + MAX_CLOCK_PPM * 1e-6),
            );
            if let Some(updated) = decisions.at(boundary + period / 2.0) {
                obs = updated;
            }
            soft = acquisition::soft(obs.power, decision_gain);
        }
        symbols.push(SymbolMetrics {
            index,
            center_sample: boundary + period / 2.0,
            bit: soft > 0.0,
            carrier_power: obs.power,
            soft_value: soft,
            confidence: soft.abs(),
            decision_ratio: decision_gain[1],
            timing_correction: correction,
        });
        boundary += period;
        if symbols.len() == PREFIX_SIZE * 8 && diagnostic_bit_count.is_none() {
            let bits: Vec<_> = symbols.iter().map(|s| s.bit).collect();
            match framing::frame_size_from_prefix(&bits_to_bytes(&bits).unwrap()) {
                Ok(size) => expected = size * 8,
                Err(error) => {
                    frame_error = Some(error);
                    break;
                }
            }
        }
    }
    let result = if let Some(error) = frame_error {
        Err(error.into())
    } else if symbols.len() < expected {
        Err(ReceiveError::Truncated {
            expected_bits: expected,
            received_bits: symbols.len(),
        })
    } else {
        let bits: Vec<_> = symbols.iter().map(|s| s.bit).collect();
        framing::decode_frame(&bits_to_bytes(&bits[..bits.len() / 8 * 8]).unwrap())
            .map_err(ReceiveError::from)
    };
    FrameAttempt {
        acquisition,
        symbols,
        final_samples_per_symbol: period,
        result,
        equalizer: None,
    }
}

/// Diagnostic-only energy demodulation past an invalid prefix. The caller sets
/// a bounded observation count; no reference bits are accepted. This cannot
/// return a packet or participate in file reassembly. Symbol timing and decisions
/// are identical to the ordinary energy path, including after bit 160.
pub fn probe_energy_symbols(
    samples: &[f32],
    acquisition: Acquisition,
    options: ReceiverOptions,
    bit_count: usize,
) -> Result<(Vec<SymbolMetrics>, f64), ReceiveError> {
    if samples.iter().any(|x| !x.is_finite()) {
        return Err(ReceiveError::InvalidSamples);
    }
    let full = ToneTrack::new(samples, samples_per_symbol());
    let half = central_track(samples, options);
    let probe = decode_candidate(
        &full,
        if options.full_symbol_decisions {
            &full
        } else {
            &half
        },
        samples.len(),
        acquisition,
        options,
        Some(bit_count),
    );
    Ok((probe.symbols, probe.final_samples_per_symbol))
}

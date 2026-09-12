//! A bounded, preamble-trained complex decision-feedback receiver for carrier
//! memory. A bounded bank is fitted and ranked using training only; whole-frame
//! framing and CRC validate each resulting hypothesis. No payload labels or bit
//! repairs are used. The ordinary energy receiver remains the first path.
use super::{Acquisition, SymbolMetrics};
use crate::{
    config::{FREQ_ONE_HZ, FREQ_ZERO_HZ, SAMPLE_RATE},
    demodulation::acquisition::training_bits,
    signal::tones::{HOP, ToneTrack},
};

const TRAINING: usize = 80;
const TAPS: usize = 9; // Direct response plus at most 80 ms of symbol memory.
const FOLDS: usize = 5;
const RIDGE: f64 = 0.1;
const STEP: f64 = 0.1;
const MIN_FEEDBACK_CONFIDENCE: f64 = 0.3;

#[derive(Debug, Clone, Copy, Default)]
struct Complex {
    re: f64,
    im: f64,
}
impl Complex {
    fn norm(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
    fn add(self, other: Self) -> Self {
        Self {
            re: self.re + other.re,
            im: self.im + other.im,
        }
    }
    fn sub(self, other: Self) -> Self {
        Self {
            re: self.re - other.re,
            im: self.im - other.im,
        }
    }
    fn scale(self, gain: f64) -> Self {
        Self {
            re: self.re * gain,
            im: self.im * gain,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EqualizerMetrics {
    pub memory_symbols: usize,
    pub samples_per_symbol: f64,
    /// Sampling displacement from the coarse acquisition, selected on training.
    pub timing_offset_samples: f64,
    /// Rank (one based) and size of the training-qualified hypothesis bank.
    pub hypothesis_rank: usize,
    pub hypothesis_count: usize,
    /// Sum of the two carrier-normalized, held-out mean squared residuals.
    pub validation_error: f64,
    pub training_errors: usize,
    pub training_min_confidence: f64,
    /// Magnitudes of initial complex channel coefficients, carrier then lag.
    pub tap_magnitudes: [Vec<f64>; 2],
}

pub(super) struct Equalizer {
    coefficients: [[Complex; TAPS]; 2],
    variance: [f64; 2],
    depth: usize,
    history: Vec<bool>,
    pub metrics: EqualizerMetrics,
}

fn design(bits: &[bool], index: usize, tone: usize, depth: usize) -> [f64; TAPS] {
    std::array::from_fn(|lag| {
        f64::from(lag <= depth && index >= lag && usize::from(bits[index - lag]) == tone)
    })
}

fn predict(h: &[Complex; TAPS], x: &[f64; TAPS]) -> Complex {
    h.iter()
        .zip(x)
        .fold(Complex::default(), |v, (&h, &x)| v.add(h.scale(x)))
}

// The ridge makes even the alternating portion of the training sequence
// nonsingular. Partial pivoting also protects very weak/degenerate observations.
fn solve(
    mut a: [[f64; TAPS]; TAPS],
    mut b: [Complex; TAPS],
    size: usize,
) -> Option<[Complex; TAPS]> {
    for col in 0..size {
        let pivot = (col..size).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let scale = a[col][col];
        for value in &mut a[col][col..size] {
            *value /= scale;
        }
        b[col] = b[col].scale(1.0 / scale);
        for row in 0..size {
            if row == col {
                continue;
            }
            let scale = a[row][col];
            let pivot_row = a[col];
            for (value, pivot_value) in a[row][col..size].iter_mut().zip(&pivot_row[col..size]) {
                *value -= scale * pivot_value;
            }
            b[row] = b[row].sub(b[col].scale(scale));
        }
    }
    Some(b)
}

fn fit(
    z: &[[Complex; 2]],
    bits: &[bool],
    tone: usize,
    depth: usize,
    held_out: Option<usize>,
) -> Option<([Complex; TAPS], f64)> {
    let included: Vec<_> = (0..TRAINING)
        .filter(|i| held_out != Some(i % FOLDS))
        .collect();
    let mut weights = [1.0; TRAINING];
    let mut h = [Complex::default(); TAPS];
    let mut scale = 0.0;
    let mut residuals = [0.0; TRAINING];
    for _ in 0..4 {
        let mut a = [[0.0; TAPS]; TAPS];
        let mut b = [Complex::default(); TAPS];
        for (i, row) in a.iter_mut().enumerate().take(depth + 1) {
            row[i] = RIDGE;
        }
        for &n in &included {
            let x = design(bits, n, tone, depth);
            for row in 0..=depth {
                b[row] = b[row].add(z[n][tone].scale(weights[n] * x[row]));
                for col in 0..=depth {
                    a[row][col] += weights[n] * x[row] * x[col];
                }
            }
        }
        h = solve(a, b, depth + 1)?;
        for &n in &included {
            residuals[n] = z[n][tone]
                .sub(predict(&h, &design(bits, n, tone, depth)))
                .norm()
                .sqrt();
        }
        let mut sorted: Vec<_> = included.iter().map(|&n| residuals[n]).collect();
        sorted.sort_by(f64::total_cmp);
        scale = (1.5 * (sorted[(sorted.len() - 1) / 2] + sorted[sorted.len() / 2]) / 2.0).max(1e-8);
        for &n in &included {
            weights[n] = (scale / residuals[n].max(1e-20)).min(1.0);
        }
    }
    let power = included.iter().map(|&n| z[n][tone].norm()).sum::<f64>() / included.len() as f64;
    let variance = included
        .iter()
        .map(|&n| residuals[n].min(2.0 * scale).powi(2))
        .sum::<f64>()
        / included.len() as f64;
    Some((h, variance.max(power * 0.01).max(1e-20)))
}

fn observation(
    track: &ToneTrack,
    acq: &Acquisition,
    index: usize,
) -> Option<([Complex; 2], [f64; 2], f64)> {
    let center = acq.start_sample + (index as f64 + 0.5) * acq.samples_per_symbol;
    let obs = track.at(center)?;
    let z = std::array::from_fn(|tone| {
        let frequency = [FREQ_ZERO_HZ, FREQ_ONE_HZ][tone] as f64;
        let phase = -std::f64::consts::TAU
            * frequency
            * (480.0 / acq.samples_per_symbol - 1.0)
            * (center - acq.start_sample)
            / SAMPLE_RATE as f64;
        let (sin, cos) = phase.sin_cos();
        Complex {
            re: (obs.real[tone] * cos - obs.imag[tone] * sin) / 120.0,
            im: (obs.real[tone] * sin + obs.imag[tone] * cos) / 120.0,
        }
    });
    Some((z, obs.power, center))
}

impl Equalizer {
    pub fn train(track: &ToneTrack, acq: &Acquisition) -> Vec<Self> {
        let mut models = Self::train_periods(track, acq);
        if acq.trained_acquisition_ratio.is_some() {
            // A full-window energy peak need not center the eye of a channel
            // with carrier memory. Select a bounded +/- half-symbol timing
            // displacement on held-out preamble/sync only, before any header
            // or CRC is read. Ordinary acquisitions keep their original timing.
            let radius = 240 / HOP as i32;
            for step in -radius..=radius {
                if step == 0 {
                    continue;
                }
                let offset = f64::from(step) * HOP as f64;
                let mut candidate = acq.clone();
                candidate.start_sample += offset;
                for mut model in Self::train_periods(track, &candidate) {
                    model.metrics.timing_offset_samples = offset;
                    models.push(model);
                }
            }
        }
        models.sort_by(|a, b| {
            a.metrics
                .validation_error
                .total_cmp(&b.metrics.validation_error)
        });
        // At most 21 offsets * 2 clock priors * 9 memory depths. Retaining
        // qualified alternatives matters when an alternating preamble cannot
        // uniquely identify the response to the later, different bit patterns.
        let count = models.len();
        for (index, model) in models.iter_mut().enumerate() {
            model.metrics.hypothesis_rank = index + 1;
            model.metrics.hypothesis_count = count;
        }
        models
    }

    fn train_periods(track: &ToneTrack, acq: &Acquisition) -> Vec<Self> {
        let mut estimated = Self::train_at(track, acq).unwrap_or_default();
        if acq.samples_per_symbol == 480.0 {
            return estimated;
        }
        // A phase slope during startup can include channel phase changes as
        // well as clock error. Validate the measured and nominal clock priors
        // on the same held-out training observations before reading a header.
        let mut nominal = acq.clone();
        nominal.samples_per_symbol = 480.0;
        estimated.extend(Self::train_at(track, &nominal).unwrap_or_default());
        estimated
    }

    fn train_at(track: &ToneTrack, acq: &Acquisition) -> Option<Vec<Self>> {
        let bits = training_bits();
        let z: Vec<_> = (0..TRAINING)
            .map(|i| observation(track, acq, i).map(|v| v.0))
            .collect::<Option<_>>()?;
        let mut models = Vec::new();
        for depth in 0..TAPS {
            let mut coefficients = [[Complex::default(); TAPS]; 2];
            let mut variance = [0.0; 2];
            let mut validation = 0.0;
            for tone in 0..2 {
                (coefficients[tone], variance[tone]) = fit(&z, &bits, tone, depth, None)?;
                let power = z.iter().map(|v| v[tone].norm()).sum::<f64>().max(1e-20);
                for fold in 0..FOLDS {
                    let (h, _) = fit(&z, &bits, tone, depth, Some(fold))?;
                    validation += (fold..TRAINING)
                        .step_by(FOLDS)
                        .map(|n| {
                            z[n][tone]
                                .sub(predict(&h, &design(&bits, n, tone, depth)))
                                .norm()
                        })
                        .sum::<f64>()
                        / power;
                }
            }
            let mut candidate = Self {
                coefficients,
                variance,
                depth,
                history: Vec::new(),
                metrics: EqualizerMetrics {
                    memory_symbols: depth,
                    samples_per_symbol: acq.samples_per_symbol,
                    timing_offset_samples: 0.0,
                    hypothesis_rank: 0,
                    hypothesis_count: 0,
                    validation_error: validation,
                    training_errors: 0,
                    training_min_confidence: 1.0,
                    tap_magnitudes: coefficients
                        .map(|h| h[..=depth].iter().map(|v| v.norm().sqrt()).collect()),
                },
            };
            for n in 0..TRAINING {
                let (bit, soft) = candidate.decide(z[n], false);
                candidate.metrics.training_errors += usize::from(bit != bits[n]);
                candidate.metrics.training_min_confidence =
                    candidate.metrics.training_min_confidence.min(soft.abs());
            }
            // Require every training bit to be *observed* correctly with actual
            // feedback decisions. Labels never replace symbols, including sync.
            // On the additional half-window acquisition path, also require
            // every training decision to meet the same confidence used for
            // adaptation. Near-silence from destructive interference must not
            // qualify solely because a fitted model weakly prefers a label.
            if candidate.metrics.training_errors == 0
                && validation < 1.0
                && (acq.acquisition_window_samples != 240
                    || candidate.metrics.training_min_confidence > MIN_FEEDBACK_CONFIDENCE)
            {
                candidate.history.clear();
                models.push(candidate);
            }
        }
        Some(models)
    }

    fn decide(&mut self, z: [Complex; 2], adapt: bool) -> (bool, f64) {
        let index = self.history.len();
        self.history.push(false);
        let mut errors = [0.0; 2];
        for (bit, error) in errors.iter_mut().enumerate() {
            self.history[index] = bit == 1;
            for (tone, &value) in z.iter().enumerate() {
                let x = design(&self.history, index, tone, self.depth);
                *error +=
                    value.sub(predict(&self.coefficients[tone], &x)).norm() / self.variance[tone];
            }
        }
        let bit = errors[1] < errors[0];
        self.history[index] = bit;
        let soft = (errors[0] - errors[1]) / (errors[0] + errors[1]).max(1e-20);
        if adapt && index >= TRAINING && soft.abs() > MIN_FEEDBACK_CONFIDENCE {
            for (tone, &value) in z.iter().enumerate() {
                let x = design(&self.history, index, tone, self.depth);
                let residual = value.sub(predict(&self.coefficients[tone], &x));
                let step = STEP / x.iter().sum::<f64>().max(1.0);
                for (h, x) in self.coefficients[tone].iter_mut().zip(x) {
                    *h = h.add(residual.scale(step * x));
                }
            }
        }
        (bit, soft)
    }

    pub fn symbol(&mut self, track: &ToneTrack, acq: &Acquisition) -> Option<SymbolMetrics> {
        let index = self.history.len();
        let mut sampling = acq.clone();
        sampling.samples_per_symbol = self.metrics.samples_per_symbol;
        sampling.start_sample += self.metrics.timing_offset_samples;
        let (z, carrier_power, center_sample) = observation(track, &sampling, index)?;
        let (bit, soft_value) = self.decide(z, true);
        Some(SymbolMetrics {
            index,
            center_sample,
            bit,
            carrier_power,
            soft_value,
            confidence: soft_value.abs(),
            decision_ratio: acq.decision_ratio_at(index),
            timing_correction: 0.0,
        })
    }
}

pub(super) fn decode(
    track: &ToneTrack,
    acquisition: Acquisition,
    mut model: Equalizer,
) -> super::FrameAttempt {
    use super::ReceiveError;
    use crate::protocol::{
        bitstream::bits_to_bytes,
        framing::{self, MAX_FRAME_SIZE, PREFIX_SIZE},
    };
    let metrics = model.metrics.clone();
    let mut symbols = Vec::new();
    let mut expected = PREFIX_SIZE * 8;
    let result = loop {
        let Some(symbol) = model.symbol(track, &acquisition) else {
            break Err(ReceiveError::Truncated {
                expected_bits: expected,
                received_bits: symbols.len(),
            });
        };
        symbols.push(symbol);
        if symbols.len() == PREFIX_SIZE * 8 {
            let bytes = bits_to_bytes(&model.history).expect("byte-aligned prefix");
            match framing::frame_size_from_prefix(&bytes) {
                Ok(size) => expected = size * 8,
                Err(error) => break Err(error.into()),
            }
        }
        if symbols.len() == expected || symbols.len() == MAX_FRAME_SIZE * 8 {
            break framing::decode_frame(
                &bits_to_bytes(&model.history).expect("byte-aligned frame"),
            )
            .map_err(ReceiveError::from);
        }
    };
    super::FrameAttempt {
        final_samples_per_symbol: model.metrics.samples_per_symbol,
        acquisition,
        symbols,
        result,
        equalizer: Some(metrics),
    }
}

use crate::{
    config::{FREQ_ONE_HZ, FREQ_ZERO_HZ, SAMPLE_RATE},
    modulation::bfsk::samples_per_symbol,
    protocol::{
        bitstream::bytes_to_bits,
        framing::{PREAMBLE, SYNC_WORD},
    },
    signal::tones::{HOP, ToneTrack},
};

pub(crate) const COARSE_STEP: usize = 120;
const MIN_QUALITY: f64 = 0.5;
const MIN_CONCENTRATION: f64 = 0.04;
/// Below -140 dBFS power, treat the input as effectively silent.
const MIN_TONE_POWER: f64 = 1e-14;
pub(crate) const MAX_CLOCK_PPM: f64 = 5000.0;
const CALIBRATION_BLOCKS: usize = 4;
const CALIBRATION_BLOCK_SYMBOLS: usize = PREAMBLE.len() * 8 / CALIBRATION_BLOCKS;

#[derive(Debug, Clone)]
pub struct Acquisition {
    /// Estimated first symbol boundary in input samples (48 kHz).
    pub start_sample: f64,
    pub samples_per_symbol: f64,
    pub preamble_matches: usize,
    pub sync_matches: usize,
    pub quality: f64,
    pub tone_concentration: f64,
    /// Median received RMS carrier powers from the known preamble.
    pub carrier_power: [f64; 2],
    /// Raw E1/E0 ratio at the calibrated decision boundary.
    pub decision_ratio: f64,
    /// Carrier-ratio estimates at the centers of four 160 ms training blocks.
    /// They are constant unless startup tracking improves preamble decisions.
    pub preamble_decision_ratios: [f64; CALIBRATION_BLOCKS],
    /// Classification errors on the 64 known preamble symbols before/after
    /// fitting the decision threshold. Carrier powers remain independent metrics.
    pub power_ratio_training_errors: usize,
    pub decision_training_errors: usize,
    pub clock_estimated: bool,
    /// If calibration invalidated an otherwise qualifying raw acquisition,
    /// retain that rejected score for diagnostics and use the raw onset.
    pub rejected_calibrated_score: Option<(usize, usize, f64)>,
    /// A preamble-trained ratio was required to discover this candidate.
    /// This is distinct from the energy decoder's carrier-power calibration.
    pub trained_acquisition_ratio: Option<f64>,
    /// Width of the observation that passed acquisition (full or central half symbol).
    pub acquisition_window_samples: usize,
}

impl Acquisition {
    pub fn decision_ratio_at(&self, symbol_index: usize) -> f64 {
        interpolate_ratio(&self.preamble_decision_ratios, symbol_index)
    }
}

fn interpolate_ratio(ratios: &[f64; CALIBRATION_BLOCKS], index: usize) -> f64 {
    // Interpolate log ratios (relative gain); hold the final trained state once
    // the known preamble ends, without using payload labels or CRC feedback.
    let position = ((index as f64 - (CALIBRATION_BLOCK_SYMBOLS - 1) as f64 / 2.0)
        / CALIBRATION_BLOCK_SYMBOLS as f64)
        .clamp(0.0, (CALIBRATION_BLOCKS - 1) as f64);
    let left = position.floor() as usize;
    let right = (left + 1).min(CALIBRATION_BLOCKS - 1);
    let fraction = position - left as f64;
    if fraction == 0.0 || ratios[left] == ratios[right] {
        return ratios[left];
    }
    (ratios[left].ln() + fraction * (ratios[right].ln() - ratios[left].ln())).exp()
}

pub(crate) fn training_bits() -> Vec<bool> {
    let mut bytes = PREAMBLE.to_vec();
    bytes.extend_from_slice(&SYNC_WORD.to_be_bytes());
    bytes_to_bits(&bytes)
}

pub(crate) fn soft(power: [f64; 2], gain: [f64; 2]) -> f64 {
    let a = power[0] / gain[0];
    let b = power[1] / gain[1];
    (b - a) / (b + a).max(1e-30)
}

fn score(
    track: &ToneTrack,
    start: f64,
    period: f64,
    bits: &[bool],
    gain: [f64; 2],
) -> Option<(usize, usize, f64, f64)> {
    let (mut preamble, mut sync, mut quality, mut concentration) = (0, 0, 0.0, 0.0);
    for (i, &bit) in bits.iter().enumerate() {
        let obs = track.at(start + (i as f64 + 0.5) * period)?;
        let decision = soft(obs.power, gain);
        if (decision > 0.0) == bit {
            if i < PREAMBLE.len() * 8 {
                preamble += 1;
            } else {
                sync += 1;
            }
        }
        quality += if bit { decision } else { -decision };
        concentration += obs.concentration;
        // Cheap rejection before scoring a full candidate throughout silence/noise.
        if i == 15 && (quality / 16.0 < 0.35 || concentration / 16.0 < MIN_CONCENTRATION) {
            return None;
        }
    }
    Some((
        preamble,
        sync,
        quality / bits.len() as f64,
        concentration / bits.len() as f64,
    ))
}

fn calibrate(track: &ToneTrack, start: f64, period: f64, bits: &[bool]) -> [f64; 2] {
    let mut values: [Vec<f64>; 2] = Default::default();
    for (i, &bit) in bits.iter().take(PREAMBLE.len() * 8).enumerate() {
        if let Some(obs) = track.at(start + (i as f64 + 0.5) * period) {
            values[usize::from(bit)].push(obs.power[usize::from(bit)]);
        }
    }
    values.map(|mut v| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2)
            .copied()
            .unwrap_or(0.0)
            .max(MIN_TONE_POWER)
    })
}

// A delayed carrier can remain present during the opposite symbol. Its active
// power alone is then a biased decision boundary. Estimate the two observed
// E1/E0 classes using only the known preamble, halfway apart in log power.
fn acquisition_ratio(track: &ToneTrack, start: f64, period: f64, bits: &[bool]) -> Option<f64> {
    let mut ratios: [Vec<f64>; 2] = Default::default();
    for (i, &bit) in bits.iter().take(64).enumerate() {
        let power = track.at(start + (i as f64 + 0.5) * period)?.power;
        ratios[usize::from(bit)].push(power[1].max(MIN_TONE_POWER) / power[0].max(MIN_TONE_POWER));
    }
    let medians = ratios.map(|mut values| {
        let middle = values.len() / 2;
        *values.select_nth_unstable_by(middle, f64::total_cmp).1
    });
    Some((medians[0] * medians[1]).sqrt())
}

/// Startup processing can change relative carrier gain during the preamble.
/// Four local median estimates (eight observations per carrier per block) track
/// that change. Retain the lower-variance global prior unless local calibration
/// strictly reduces known-preamble errors. No observed bit is replaced by a label.
fn decision_threshold(
    track: &ToneTrack,
    start: f64,
    period: f64,
    bits: &[bool],
    prior: f64,
    train: bool,
) -> ([f64; CALIBRATION_BLOCKS], usize, usize) {
    let training: Vec<_> = bits
        .iter()
        .take(PREAMBLE.len() * 8)
        .enumerate()
        .filter_map(|(i, &bit)| {
            track
                .at(start + (i as f64 + 0.5) * period)
                .map(|obs| (i, obs.power, bit))
        })
        .collect();
    let prior_errors = training
        .iter()
        .filter(|&&(_, power, bit)| (soft(power, [1.0, prior]) > 0.0) != bit)
        .count();
    if !train || prior_errors == 0 {
        return ([prior; CALIBRATION_BLOCKS], prior_errors, prior_errors);
    }
    let ratios = std::array::from_fn(|block| {
        let first = block * CALIBRATION_BLOCK_SYMBOLS;
        let power = calibrate(
            track,
            start + first as f64 * period,
            period,
            &bits[first..first + CALIBRATION_BLOCK_SYMBOLS],
        );
        power[1] / power[0]
    });
    let errors = training
        .iter()
        .filter(|&&(i, power, bit)| {
            (soft(power, [1.0, interpolate_ratio(&ratios, i)]) > 0.0) != bit
        })
        .count();
    if errors < prior_errors {
        (ratios, prior_errors, errors)
    } else {
        ([prior; CALIBRATION_BLOCKS], prior_errors, prior_errors)
    }
}

/// The V0 transmitter has continuous phase and integer cycles per symbol.
/// Phase slope across repeated preamble carriers estimates a common sample
/// clock ratio without accumulating noisy symbol-edge errors. Use both carriers;
/// reject inconsistent fits. No reference waveform or payload is used.
fn clock_period(track: &ToneTrack, start: f64, bits: &[bool]) -> Option<f64> {
    let nominal = samples_per_symbol() as f64;
    let mut estimates = Vec::new();
    for (tone, frequency) in [FREQ_ZERO_HZ, FREQ_ONE_HZ].into_iter().enumerate() {
        let mut points = Vec::new();
        let (mut previous, mut unwrapped) = (None, 0.0);
        for (i, &bit) in bits.iter().take(64).enumerate().skip(2) {
            if usize::from(bit) != tone {
                continue;
            }
            let t = start + (i as f64 + 0.5) * nominal;
            let obs = track.at(t)?;
            let angle = obs.imag[tone].atan2(obs.real[tone]);
            if let Some(last) = previous {
                let delta: f64 = angle - last;
                unwrapped += (delta + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                    - std::f64::consts::PI;
            }
            previous = Some(angle);
            points.push((t - start, unwrapped, obs.power[tone]));
        }
        let weight: f64 = points.iter().map(|p| p.2).sum();
        if weight < MIN_TONE_POWER {
            return None;
        }
        let mean_x = points.iter().map(|p| p.0 * p.2).sum::<f64>() / weight;
        let mean_y = points.iter().map(|p| p.1 * p.2).sum::<f64>() / weight;
        let slope = points
            .iter()
            .map(|p| p.2 * (p.0 - mean_x) * (p.1 - mean_y))
            .sum::<f64>()
            / points
                .iter()
                .map(|p| p.2 * (p.0 - mean_x).powi(2))
                .sum::<f64>();
        let residual = (points
            .iter()
            .map(|p| p.2 * (p.1 - mean_y - slope * (p.0 - mean_x)).powi(2))
            .sum::<f64>()
            / weight)
            .sqrt();
        let ratio =
            1.0 / (1.0 + slope / (std::f64::consts::TAU * frequency as f64 / SAMPLE_RATE as f64));
        if !ratio.is_finite()
            || residual > 0.35
            || (ratio - 1.0).abs() * 1e6 > MAX_CLOCK_PPM + 200.0
        {
            return None;
        }
        estimates.push((ratio, weight));
    }
    if (estimates[0].0 - estimates[1].0).abs() > 0.001 {
        return None;
    }
    Some(
        nominal * estimates.iter().map(|p| p.0 * p.1).sum::<f64>()
            / estimates.iter().map(|p| p.1).sum::<f64>(),
    )
}

pub(crate) fn acquire(
    full: &ToneTrack,
    half: &ToneTrack,
    sample_count: usize,
    calibration: bool,
    clock_recovery: bool,
    train_threshold: bool,
    preserve_raw_candidate: bool,
) -> Vec<Acquisition> {
    let bits = training_bits();
    let period = samples_per_symbol() as f64;
    let Some(last) = sample_count.checked_sub(bits.len() * samples_per_symbol()) else {
        return Vec::new();
    };
    let mut peaks: Vec<(f64, f64)> = Vec::new();
    for start in (0..=last).step_by(COARSE_STEP) {
        let Some((preamble, sync, quality, _)) = score(full, start as f64, period, &bits, [1.0; 2])
        else {
            continue;
        };
        if preamble < 56 || sync < 13 || quality < MIN_QUALITY {
            continue;
        }
        if let Some(peak) = peaks
            .last_mut()
            .filter(|p| start as f64 - p.0 < period * 64.0)
        {
            if quality > peak.1 {
                *peak = (start as f64, quality);
            }
        } else {
            peaks.push((start as f64, quality));
        }
    }
    let mut peaks: Vec<_> = peaks.into_iter().map(|(s, q)| (s, q, None, 480)).collect();
    if calibration && preserve_raw_candidate {
        // Preserve the established full-symbol path first. Reflections can
        // mix adjacent symbols across its edges while the central half still
        // resolves the known sequence. Apply the SAME gates on that window,
        // with a finer grid for narrow peaks, only where no full-window peak
        // already qualifies. Neither search uses header or payload labels.
        for (track, width) in [(full, 480), (half, 240)] {
            let mut extra: Vec<(f64, f64, Option<f64>, usize)> = Vec::new();
            for start in (0..=last).step_by(if width == 240 { HOP / 2 } else { HOP }) {
                let start = start as f64;
                if peaks.iter().any(|p| (start - p.0).abs() < period * 64.0) {
                    continue;
                }
                let Some(ratio) = acquisition_ratio(track, start, period, &bits) else {
                    continue;
                };
                let Some((preamble, sync, quality, concentration)) =
                    score(track, start, period, &bits, [1.0, ratio])
                else {
                    continue;
                };
                if preamble < 56
                    || sync < 13
                    || quality < MIN_QUALITY
                    || concentration < MIN_CONCENTRATION
                {
                    continue;
                }
                if let Some(peak) = extra.last_mut().filter(|p| start - p.0 < period * 64.0) {
                    if quality > peak.1 {
                        *peak = (start, quality, Some(ratio), width);
                    }
                } else {
                    extra.push((start, quality, Some(ratio), width));
                }
            }
            peaks.extend(extra);
        }
        peaks.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    peaks
        .into_iter()
        .filter_map(
            |(coarse_start, _, trained_ratio, acquisition_window_samples)| {
                let acquisition_track = if acquisition_window_samples == 240 {
                    half
                } else {
                    full
                };
                let estimated = clock_recovery
                    .then(|| clock_period(half, coarse_start, &bits))
                    .flatten();
                let period = estimated.unwrap_or(period);
                let power = calibrate(half, coarse_start, period, &bits);
                if power.iter().any(|&p| p <= MIN_TONE_POWER) {
                    return None;
                }
                let gain = trained_ratio.map_or_else(
                    || if calibration { power } else { [1.0; 2] },
                    |ratio| [1.0, ratio],
                );
                let mut best = None;
                // Include the accumulated preamble displacement when refining the onset.
                for delta in -80..=80 {
                    let start = coarse_start + delta as f64 * 3.0;
                    if start < 0.0 {
                        continue;
                    }
                    if let Some(metrics) = score(acquisition_track, start, period, &bits, gain)
                        && best
                            .as_ref()
                            .is_none_or(|(_, m): &(f64, (usize, usize, f64, f64))| metrics.2 > m.2)
                    {
                        best = Some((start, metrics));
                    }
                }
                let (mut start, (mut preamble, mut sync, mut quality, mut concentration)) = best?;
                let mut rejected_calibrated_score = None;
                if preamble < 56
                    || sync < 13
                    || quality < MIN_QUALITY
                    || concentration < MIN_CONCENTRATION
                {
                    if !preserve_raw_candidate {
                        return None;
                    }
                    rejected_calibrated_score = Some((preamble, sync, quality));
                    // Calibration is a channel hypothesis, not grounds to erase a
                    // raw candidate that independently meets the SAME acquisition
                    // requirements. Framing and CRC still have to validate it.
                    start = coarse_start;
                    (preamble, sync, quality, concentration) = score(
                        acquisition_track,
                        start,
                        period,
                        &bits,
                        trained_ratio.map_or([1.0; 2], |r| [1.0, r]),
                    )?;
                    if preamble < 56
                        || sync < 13
                        || quality < MIN_QUALITY
                        || concentration < MIN_CONCENTRATION
                    {
                        return None;
                    }
                }
                let power = calibrate(half, start, period, &bits);
                let (
                    preamble_decision_ratios,
                    power_ratio_training_errors,
                    decision_training_errors,
                ) = decision_threshold(
                    half,
                    start,
                    period,
                    &bits,
                    if calibration {
                        power[1] / power[0]
                    } else {
                        1.0
                    },
                    calibration && train_threshold,
                );
                Some(Acquisition {
                    start_sample: start,
                    samples_per_symbol: period,
                    preamble_matches: preamble,
                    sync_matches: sync,
                    quality,
                    tone_concentration: concentration,
                    carrier_power: power,
                    decision_ratio: preamble_decision_ratios[CALIBRATION_BLOCKS - 1],
                    preamble_decision_ratios,
                    power_ratio_training_errors,
                    decision_training_errors,
                    clock_estimated: estimated.is_some(),
                    rejected_calibrated_score,
                    trained_acquisition_ratio: trained_ratio,
                    acquisition_window_samples,
                })
            },
        )
        .collect()
}

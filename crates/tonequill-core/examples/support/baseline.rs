use tonequill_core::{
    config::{FREQ_ONE_HZ, FREQ_ZERO_HZ, SAMPLE_RATE},
    modulation::bfsk::samples_per_symbol,
    protocol::{bitstream::bytes_to_bits, framing::PREAMBLE},
};

use tonequill_core::demodulation::goertzel;

#[derive(Debug, Clone, Copy)]
pub struct SyncResult {
    pub start_sample: usize,
    pub matched_bits: usize,
    pub tested_bits: usize,
    pub quality: f32,
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let energy: f32 = samples.iter().map(|sample| sample * sample).sum();

    (energy / samples.len() as f32).sqrt()
}

fn detect_signal_onset(samples: &[f32]) -> usize {
    if samples.is_empty() {
        return 0;
    }

    let noise_sample_count = (SAMPLE_RATE as usize / 50).min(samples.len());

    let noise_level = rms(&samples[..noise_sample_count]);

    let threshold = (noise_level * 5.0).max(0.005);

    let window_size = (SAMPLE_RATE as usize / 200).max(1);

    let step = (window_size / 2).max(1);

    let mut start = 0usize;

    while start + window_size <= samples.len() {
        let window = &samples[start..start + window_size];

        if rms(window) > threshold {
            return start.saturating_sub(window_size);
        }

        start += step;
    }

    0
}

fn preamble_score(samples: &[f32], candidate_start: usize, expected_bits: &[bool]) -> (usize, f32) {
    let symbol_size = samples_per_symbol();

    let mut matches = 0usize;

    let mut total_quality = 0.0_f32;

    for (index, &expected_bit) in expected_bits.iter().enumerate() {
        let start = candidate_start + index * symbol_size;

        let end = start + symbol_size;

        if end > samples.len() {
            break;
        }

        let symbol = &samples[start..end];

        let zero_power = goertzel::power(symbol, FREQ_ZERO_HZ, SAMPLE_RATE);

        let one_power = goertzel::power(symbol, FREQ_ONE_HZ, SAMPLE_RATE);

        let detected_bit = one_power > zero_power;

        if detected_bit == expected_bit {
            matches += 1;
        }

        let expected_power = if expected_bit { one_power } else { zero_power };

        let wrong_power = if expected_bit { zero_power } else { one_power };

        let denominator = expected_power + wrong_power + 1e-12;

        let quality = (expected_power - wrong_power) / denominator;

        total_quality += quality;
    }

    let average_quality = total_quality / expected_bits.len() as f32;

    (matches, average_quality)
}

pub fn find_frame_start(samples: &[f32]) -> Option<SyncResult> {
    let symbol_size = samples_per_symbol();

    let expected_preamble = bytes_to_bits(&PREAMBLE);

    let tested_bits = 32.min(expected_preamble.len());

    let expected = &expected_preamble[..tested_bits];

    let onset = detect_signal_onset(samples);

    let radius = symbol_size * 2;

    let search_start = onset.saturating_sub(radius);

    let required_samples = tested_bits * symbol_size;

    if samples.len() < required_samples {
        return None;
    }

    let latest_start = samples.len() - required_samples;

    let search_end = (onset + radius).min(latest_start);

    let mut best_start = search_start;

    let mut best_matches = 0usize;

    let mut best_quality = f32::NEG_INFINITY;

    const COARSE_STEP: usize = 4;

    for candidate in (search_start..=search_end).step_by(COARSE_STEP) {
        let (matches, quality) = preamble_score(samples, candidate, expected);

        if quality > best_quality {
            best_start = candidate;
            best_matches = matches;
            best_quality = quality;
        }
    }

    let refine_start = best_start.saturating_sub(COARSE_STEP);

    let refine_end = (best_start + COARSE_STEP).min(latest_start);

    for candidate in refine_start..=refine_end {
        let (matches, quality) = preamble_score(samples, candidate, expected);

        if quality > best_quality {
            best_start = candidate;
            best_matches = matches;
            best_quality = quality;
        }
    }

    let minimum_matches = (tested_bits * 7) / 8;

    if best_matches < minimum_matches {
        return None;
    }

    if best_quality < 0.50 {
        return None;
    }

    Some(SyncResult {
        start_sample: best_start,
        matched_bits: best_matches,
        tested_bits,
        quality: best_quality,
    })
}

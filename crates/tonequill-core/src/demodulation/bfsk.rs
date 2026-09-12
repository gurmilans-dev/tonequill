//! Nominal, aligned Goertzel demodulation utilities for DSP tests and legacy
//! comparisons. Real recordings should use the calibrated receiver API.
use crate::config::{FREQ_ONE_HZ, FREQ_ZERO_HZ, SAMPLE_RATE};

use crate::modulation::bfsk::samples_per_symbol;

use super::goertzel;

pub fn detect_symbol(samples: &[f32]) -> bool {
    analyze_symbol(samples).bit
}
pub fn demodulate_from_offset(samples: &[f32], start_sample: usize) -> Vec<bool> {
    if start_sample >= samples.len() {
        return Vec::new();
    }

    let symbol_size = samples_per_symbol();

    samples[start_sample..]
        .chunks_exact(symbol_size)
        .map(detect_symbol)
        .collect()
}

pub fn demodulate(samples: &[f32]) -> Vec<bool> {
    demodulate_from_offset(samples, 0)
}
#[derive(Debug, Clone, Copy)]
pub struct SymbolDecision {
    pub bit: bool,
    pub zero_power: f32,
    pub one_power: f32,
    pub confidence: f32,
}
pub fn analyze_symbol(samples: &[f32]) -> SymbolDecision {
    let zero_power = goertzel::power(samples, FREQ_ZERO_HZ, SAMPLE_RATE);

    let one_power = goertzel::power(samples, FREQ_ONE_HZ, SAMPLE_RATE);

    let bit = one_power > zero_power;

    let total_power = zero_power + one_power + 1e-12;

    let confidence = (one_power - zero_power).abs() / total_power;

    SymbolDecision {
        bit,
        zero_power,
        one_power,
        confidence,
    }
}

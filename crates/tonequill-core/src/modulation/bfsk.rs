use std::f32::consts::PI;

use crate::config::{AMPLITUDE, FREQ_ONE_HZ, FREQ_ZERO_HZ, SAMPLE_RATE, SYMBOL_DURATION_MS};

pub fn samples_per_symbol() -> usize {
    ((SAMPLE_RATE as u64 * SYMBOL_DURATION_MS as u64) / 1000) as usize
}

pub fn modulate(bits: &[bool]) -> Vec<f32> {
    let samples_per_symbol = samples_per_symbol();

    let mut output = Vec::with_capacity(bits.len() * samples_per_symbol);

    let mut phase = 0.0_f32;

    for &bit in bits {
        let frequency = if bit { FREQ_ONE_HZ } else { FREQ_ZERO_HZ };

        let phase_increment = 2.0 * PI * frequency / SAMPLE_RATE as f32;

        for _ in 0..samples_per_symbol {
            let sample = AMPLITUDE * phase.sin();

            output.push(sample);

            phase += phase_increment;

            if phase >= 2.0 * PI {
                phase -= 2.0 * PI;
            }
        }
    }

    output
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_expected_number_of_samples() {
        let bits = [true, false, true, true, false, false, true, false];

        let samples = modulate(&bits);

        assert_eq!(samples.len(), bits.len() * samples_per_symbol());
    }
}

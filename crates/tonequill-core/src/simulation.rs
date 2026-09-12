//! Deterministic, offline acoustic-channel experiments. SNR is signal RMS power
//! over the active (resampled, echoed) waveform / broadband white-noise power;
//! silence padding does not change the requested SNR. No clipping is applied.

use crate::{config::*, modulation::bfsk::samples_per_symbol};
use thiserror::Error;

/// An additional delayed path, with an optional linear change in its amplitude.
#[derive(Debug, Clone)]
pub struct Echo {
    pub delay_samples: usize,
    pub gain: f64,
    pub end_gain_ratio: f64,
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub seed: u64,
    pub leading_samples: usize,
    pub trailing_samples: usize,
    pub gain: f64,
    /// End/start amplitude ratio: a linear gain ramp during the transmission.
    pub end_gain_ratio: f64,
    pub one_carrier_gain: f64,
    /// Optional startup model: the one carrier ramps from this amplitude gain
    /// to one_carrier_gain, then holds. A zero duration disables the ramp.
    pub one_carrier_start_gain: f64,
    pub carrier_ramp_symbols: usize,
    pub phase_radians: f64,
    /// Positive ppm makes the received waveform longer and its pitch lower.
    pub clock_ppm: f64,
    pub echo_delay_samples: usize,
    pub echo_gain: f64,
    pub echoes: Vec<Echo>,
    pub snr_db: Option<f64>,
    pub dc_offset: f64,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            seed: 1,
            leading_samples: 0,
            trailing_samples: 0,
            gain: 1.0,
            end_gain_ratio: 1.0,
            one_carrier_gain: 1.0,
            one_carrier_start_gain: 1.0,
            carrier_ramp_symbols: 0,
            phase_radians: 0.0,
            clock_ppm: 0.0,
            echo_delay_samples: 0,
            echo_gain: 0.0,
            echoes: Vec::new(),
            snr_db: None,
            dc_offset: 0.0,
        }
    }
}

#[derive(Debug, Error)]
#[error("invalid synthetic channel parameters")]
pub struct ChannelError;

/// Fixed SplitMix64 generator and Box-Muller noise, independent of external RNG
/// crate versions. The second Gaussian draw is intentionally discarded.
pub struct Noise {
    state: u64,
}
impl Noise {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    pub fn uniform(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^= z >> 31;
        ((z >> 11) as f64 + 0.5) / (1_u64 << 53) as f64
    }
    pub fn gaussian(&mut self) -> f64 {
        (-2.0 * self.uniform().ln()).sqrt() * (std::f64::consts::TAU * self.uniform()).cos()
    }
}

impl Channel {
    pub fn transmit(&self, bits: &[bool]) -> Result<Vec<f32>, ChannelError> {
        let finite = [
            self.gain,
            self.end_gain_ratio,
            self.one_carrier_gain,
            self.one_carrier_start_gain,
            self.phase_radians,
            self.clock_ppm,
            self.echo_gain,
            self.dc_offset,
        ];
        if finite.iter().any(|v| !v.is_finite())
            || self.echoes.iter().any(|e| {
                !e.gain.is_finite() || !e.end_gain_ratio.is_finite() || e.end_gain_ratio < 0.0
            })
            || self.gain < 0.0
            || self.end_gain_ratio < 0.0
            || self.one_carrier_gain <= 0.0
            || self.one_carrier_start_gain <= 0.0
            || self.clock_ppm.abs() > 50_000.0
            || self
                .snr_db
                .is_some_and(|v| !v.is_finite() || v.abs() > 150.0)
        {
            return Err(ChannelError);
        }
        let symbol_size = samples_per_symbol();
        let count = bits.len().checked_mul(symbol_size).ok_or(ChannelError)?;
        let mut clean = Vec::with_capacity(count);
        let mut phase = self.phase_radians;
        for &bit in bits {
            let frequency = if bit { FREQ_ONE_HZ } else { FREQ_ZERO_HZ } as f64;
            for _ in 0..symbol_size {
                let startup = if self.carrier_ramp_symbols == 0 {
                    1.0
                } else {
                    (clean.len() as f64 / (self.carrier_ramp_symbols as f64 * symbol_size as f64))
                        .min(1.0)
                };
                let carrier_gain = if bit {
                    if startup == 1.0 {
                        self.one_carrier_gain
                    } else {
                        self.one_carrier_start_gain
                            + startup * (self.one_carrier_gain - self.one_carrier_start_gain)
                    }
                } else {
                    1.0
                };
                let ramp =
                    1.0 + (self.end_gain_ratio - 1.0) * clean.len() as f64 / count.max(1) as f64;
                clean.push(
                    (phase.sin() * AMPLITUDE as f64 * self.gain * carrier_gain * ramp) as f32,
                );
                phase = (phase + std::f64::consts::TAU * frequency / SAMPLE_RATE as f64)
                    % std::f64::consts::TAU;
            }
        }
        self.distort(&clean)
    }

    fn distort(&self, clean: &[f32]) -> Result<Vec<f32>, ChannelError> {
        let stretch = 1.0 + self.clock_ppm * 1e-6;
        let count = (clean.len() as f64 * stretch).ceil() as usize;
        let echo_tail = if self.echo_gain == 0.0 {
            0
        } else {
            self.echo_delay_samples
        }
        .max(
            self.echoes
                .iter()
                .filter(|e| e.gain != 0.0)
                .map(|e| e.delay_samples)
                .max()
                .unwrap_or(0),
        );
        let active_count = count.checked_add(echo_tail).ok_or(ChannelError)?;
        let total = active_count
            .checked_add(self.leading_samples)
            .and_then(|n| n.checked_add(self.trailing_samples))
            .ok_or(ChannelError)?;
        let mut active = vec![0.0_f32; active_count];
        for i in 0..count {
            let position = i as f64 / stretch;
            let left = position.floor() as usize;
            let fraction = (position - left as f64) as f32;
            let a = clean.get(left).copied().unwrap_or(0.0);
            let b = clean.get(left + 1).copied().unwrap_or(a);
            let value = a + fraction * (b - a);
            active[i] += value;
            if self.echo_gain != 0.0 {
                active[i + self.echo_delay_samples] += value * self.echo_gain as f32;
            }
            for echo in &self.echoes {
                if echo.gain != 0.0 {
                    let ramp = 1.0 + (echo.end_gain_ratio - 1.0) * i as f64 / count.max(1) as f64;
                    active[i + echo.delay_samples] += value * (echo.gain * ramp) as f32;
                }
            }
        }
        let power =
            active.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / active.len().max(1) as f64;
        let sigma = self
            .snr_db
            .map_or(0.0, |snr| (power / 10_f64.powf(snr / 10.0)).sqrt());
        let mut rng = Noise::new(self.seed);
        let mut result = Vec::with_capacity(total);
        for i in 0..total {
            let signal = i
                .checked_sub(self.leading_samples)
                .and_then(|j| active.get(j))
                .copied()
                .unwrap_or(0.0);
            let noise = if sigma > 0.0 {
                sigma * rng.gaussian()
            } else {
                0.0
            };
            result.push((signal as f64 + self.dc_offset + noise) as f32);
        }
        Ok(result)
    }
}

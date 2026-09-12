//! Sliding quadrature correlators at the two V0 carriers. Both carriers complete
//! an integer number of cycles in 240 samples. Rectangular 240/480-sample
//! windows therefore reject constant DC and the other carrier without a filter.
//! f64 accumulators avoid drift in long recordings; default observations are
//! every 0.5 ms, with a configurable finer grid for central-window acquisition.
use crate::config::{FREQ_ONE_HZ, FREQ_ZERO_HZ, SAMPLE_RATE};

pub(crate) const HOP: usize = 24;
const OSCILLATOR_PERIOD: usize = 240;

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Observation {
    pub power: [f64; 2],
    pub real: [f64; 2],
    pub imag: [f64; 2],
    pub concentration: f64,
}

pub(crate) struct ToneTrack {
    window: usize,
    hop: usize,
    frames: Vec<Observation>,
}

impl ToneTrack {
    pub fn new(samples: &[f32], window: usize) -> Self {
        Self::with_hop(samples, window, HOP)
    }

    pub fn with_hop(samples: &[f32], window: usize, hop: usize) -> Self {
        assert!(window.is_multiple_of(OSCILLATOR_PERIOD));
        assert!(hop > 0);
        let oscillators: Vec<_> = (0..OSCILLATOR_PERIOD)
            .map(|i| {
                [FREQ_ZERO_HZ, FREQ_ONE_HZ].map(|f| {
                    let angle = std::f64::consts::TAU * f as f64 * i as f64 / SAMPLE_RATE as f64;
                    (angle.cos(), -angle.sin())
                })
            })
            .collect();
        let mut frames = Vec::with_capacity(samples.len() / hop);
        let (mut real, mut imag) = ([0.0; 2], [0.0; 2]);
        let (mut sum, mut square_sum) = (0.0, 0.0);
        for (i, &sample) in samples.iter().enumerate() {
            let x = sample as f64;
            let old = if i >= window {
                samples[i - window] as f64
            } else {
                0.0
            };
            let delta = x - old;
            for tone in 0..2 {
                let (cos, sin) = oscillators[i % OSCILLATOR_PERIOD][tone];
                real[tone] += delta * cos;
                imag[tone] += delta * sin;
            }
            sum += delta;
            square_sum += x * x - old * old;
            if i + 1 >= window && (i + 1 - window).is_multiple_of(hop) {
                let scale = 2.0 / (window * window) as f64;
                let power =
                    std::array::from_fn(|tone| (real[tone].powi(2) + imag[tone].powi(2)) * scale);
                let variance =
                    (square_sum / window as f64 - (sum / window as f64).powi(2)).max(0.0);
                frames.push(Observation {
                    power,
                    real,
                    imag,
                    concentration: ((power[0] + power[1]) / variance.max(1e-30)).clamp(0.0, 1.0),
                });
            }
        }
        Self {
            window,
            hop,
            frames,
        }
    }

    pub fn at(&self, center: f64) -> Option<Observation> {
        let position = (center - self.window as f64 / 2.0) / self.hop as f64;
        if position < 0.0 || !position.is_finite() {
            return None;
        }
        let left = position.floor() as usize;
        let a = *self.frames.get(left)?;
        let fraction = position - left as f64;
        if fraction < 1e-9 {
            return Some(a);
        }
        let b = *self.frames.get(left + 1)?;
        let mix = |x: f64, y: f64| x + fraction * (y - x);
        Some(Observation {
            power: std::array::from_fn(|i| mix(a.power[i], b.power[i])),
            real: std::array::from_fn(|i| mix(a.real[i], b.real[i])),
            imag: std::array::from_fn(|i| mix(a.imag[i], b.imag[i])),
            concentration: mix(a.concentration, b.concentration),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demodulation::goertzel;

    #[test]
    fn dense_half_windows_match_direct_correlators_across_transitions() {
        let samples: Vec<f32> = (0..3000)
            .map(|i| {
                let frequency = if (i / 137) % 2 == 0 { 1200.0 } else { 2200.0 };
                (0.1 * (std::f64::consts::TAU * frequency * i as f64 / SAMPLE_RATE as f64).sin())
                    as f32
            })
            .collect();
        let track = ToneTrack::with_hop(&samples, 240, 12);
        for start in [12, 84, 252, 2412] {
            let observation = track.at((start + 120) as f64).unwrap();
            for (tone, frequency) in [FREQ_ZERO_HZ, FREQ_ONE_HZ].into_iter().enumerate() {
                let direct = goertzel::power(&samples[start..start + 240], frequency, SAMPLE_RATE)
                    as f64
                    * 2.0
                    / (240 * 240) as f64;
                assert!((observation.power[tone] - direct).abs() < 1e-7);
            }
        }
    }

    #[test]
    fn sliding_powers_match_direct_correlators_after_long_dc_input() {
        let samples: Vec<_> = (0..200000)
            .map(|i| {
                let t = i as f64 / SAMPLE_RATE as f64;
                (0.17
                    + 0.03 * (std::f64::consts::TAU * 1200.0 * t).sin()
                    + 0.007 * (std::f64::consts::TAU * 2200.0 * t + 1.1).sin())
                    as f32
            })
            .collect();
        for window in [240, 480] {
            let track = ToneTrack::new(&samples, window);
            for start in [0, 24000, 192000] {
                let obs = track.at((start + window / 2) as f64).unwrap();
                for (tone, frequency) in [FREQ_ZERO_HZ, FREQ_ONE_HZ].into_iter().enumerate() {
                    let direct =
                        goertzel::power(&samples[start..start + window], frequency, SAMPLE_RATE)
                            as f64
                            * 2.0
                            / (window * window) as f64;
                    assert!((obs.power[tone] - direct).abs() < 1e-7);
                }
                assert!(obs.concentration > 0.999);
            }
        }
    }
}

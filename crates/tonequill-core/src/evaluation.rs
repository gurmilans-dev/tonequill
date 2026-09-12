//! Reproducible, optional Monte Carlo evaluation; no file or console I/O.
use crate::{
    config::MAX_PAYLOAD_SIZE,
    protocol::{
        bitstream::bytes_to_bits,
        framing::{FrameError, encode_frame},
        packet::Packet,
    },
    receiver::{ReceiveError, ReceiverOptions, compare_bits, inspect},
    simulation::{Channel, ChannelError, Noise},
};

#[derive(Debug, Clone)]
pub struct EvaluationConfig {
    pub trials: usize,
    pub seed: u64,
    pub payload_bytes: usize,
    pub channel: Channel,
    pub receiver: ReceiverOptions,
}

#[derive(Debug, Default)]
pub struct EvaluationStats {
    pub total_packets: usize,
    pub valid_packets: usize,
    pub total_bits: usize,
    pub compared_bits: usize,
    pub bit_errors: usize,
    pub missing_bits: usize,
    pub acquisition_failures: usize,
    pub crc_failures: usize,
    pub protocol_failures: usize,
    pub truncated_frames: usize,
    /// Must stay zero. A CRC-valid packet differing from TX is an integrity failure.
    pub incorrect_valid_packets: usize,
}
impl EvaluationStats {
    pub fn ber(&self) -> Option<f64> {
        (self.compared_bits > 0).then(|| self.bit_errors as f64 / self.compared_bits as f64)
    }
    pub fn per(&self) -> Option<f64> {
        (self.total_packets > 0)
            .then(|| 1.0 - self.valid_packets as f64 / self.total_packets as f64)
    }
}

pub fn evaluate(config: &EvaluationConfig) -> Result<EvaluationStats, ChannelError> {
    if config.trials == 0 || config.payload_bytes > MAX_PAYLOAD_SIZE {
        return Err(ChannelError);
    }
    let mut stats = EvaluationStats::default();
    for trial in 0..config.trials {
        let seed = config.seed.wrapping_add(trial as u64);
        let mut rng = Noise::new(seed ^ 0xa0b1_c2d3_e4f5_6789);
        let payload = (0..config.payload_bytes)
            .map(|_| (rng.uniform() * 256.0) as u8)
            .collect();
        let packet = Packet::new(seed as u32, trial as u16, payload);
        let tx = bytes_to_bits(&encode_frame(&packet).expect("validated V0 payload size"));
        let samples = Channel {
            seed,
            ..config.channel.clone()
        }
        .transmit(&tx)?;
        stats.total_packets += 1;
        stats.total_bits += tx.len();
        match inspect(&samples, config.receiver) {
            Err(ReceiveError::AcquisitionFailed) => {
                stats.acquisition_failures += 1;
                stats.missing_bits += tx.len();
            }
            Err(_) => return Err(ChannelError),
            Ok(attempts) => {
                let attempt = attempts
                    .iter()
                    .find(|a| a.result.is_ok())
                    .unwrap_or(&attempts[0]);
                let bits = compare_bits(&tx, &attempt.bits());
                stats.compared_bits += bits.compared_bits;
                stats.bit_errors += bits.bit_errors;
                stats.missing_bits += bits.missing_bits;
                match &attempt.result {
                    Ok(received) if *received == packet => stats.valid_packets += 1,
                    Ok(_) => stats.incorrect_valid_packets += 1,
                    Err(ReceiveError::Frame(FrameError::CrcMismatch { .. })) => {
                        stats.crc_failures += 1
                    }
                    Err(ReceiveError::Truncated { .. }) => stats.truncated_frames += 1,
                    Err(_) => stats.protocol_failures += 1,
                }
            }
        }
    }
    Ok(stats)
}

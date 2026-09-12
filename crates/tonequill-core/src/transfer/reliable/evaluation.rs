//! Repeatable protocol campaigns, using consecutive seeds and identical seeded
//! payloads across strategies. Aggregate goodput includes time spent on failures.
use super::{
    ReliableError,
    simulation::{SimulationConfig, simulate},
};
use crate::{simulation::Noise, transfer::MAX_FILE_SIZE};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CampaignResult {
    pub trials: u32,
    pub successful: u32,
    pub receiver_committed: u32,
    pub sender_confirmed: u32,
    pub data_frames: u64,
    pub metadata_frames: u64,
    pub control_frames: u64,
    pub complete_frames: u64,
    pub retransmitted_data: u64,
    pub timeouts: u64,
    pub retries: u64,
    pub retry_exhausted: u32,
    pub delivered_frames: u64,
    pub unique_data_packets: u64,
    pub dropped_frames: u64,
    pub crc_rejects: u64,
    pub half_duplex_rejects: u64,
    pub useful_bytes: u64,
    pub on_air_bits: u64,
    pub modulation_ms: u64,
    pub guard_ms: u64,
    pub elapsed_ms: u64,
    pub successful_elapsed_ms: u64,
}
impl CampaignResult {
    pub fn completion_rate(&self) -> f64 {
        self.successful as f64 / self.trials as f64
    }
    pub fn goodput_bytes_per_second(&self) -> f64 {
        self.useful_bytes as f64 * 1000.0 / self.elapsed_ms as f64
    }
    /// Attempted on-air bits, including retransmissions and control, per wall-time.
    pub fn throughput_bits_per_second(&self) -> f64 {
        self.on_air_bits as f64 * 1000.0 / self.elapsed_ms as f64
    }
    /// Fraction of transmitted bits that did not become original useful file data
    /// in a successful transfer. Includes framing, control, retries and failed runs.
    pub fn overhead_fraction(&self) -> f64 {
        1.0 - self.useful_bytes as f64 * 8.0 / self.on_air_bits as f64
    }
}

pub fn campaign(
    file_size: usize,
    trials: u32,
    first_seed: u64,
    config: &SimulationConfig,
) -> Result<CampaignResult, ReliableError> {
    config.validate()?;
    if trials == 0
        || trials > 100_000
        || file_size as u64 > MAX_FILE_SIZE
        || first_seed.checked_add(u64::from(trials - 1)).is_none()
    {
        return Err(ReliableError::State(
            "campaign size, trials or seed range outside bounds",
        ));
    }
    let mut totals = CampaignResult::default();
    for index in 0..trials {
        let seed = first_seed + u64::from(index);
        let mut random = Noise::new(seed ^ 0x534c_6172_f11e_0a01);
        let data: Vec<u8> = (0..file_size)
            .map(|_| (random.uniform() * 256.0) as u8)
            .collect();
        let result = simulate(
            &data,
            &SimulationConfig {
                seed,
                ..config.clone()
            },
        )?;
        totals.trials += 1;
        totals.successful += u32::from(result.success);
        totals.receiver_committed += u32::from(result.receiver_committed);
        totals.sender_confirmed += u32::from(result.sender_confirmed);
        totals.data_frames += result.data_frames;
        totals.metadata_frames += result.metadata_frames;
        totals.control_frames += result.control_frames;
        totals.complete_frames += result.complete_frames;
        totals.retransmitted_data += u64::from(result.sender.retransmitted_data);
        totals.timeouts += u64::from(result.sender.timeouts);
        totals.retries += u64::from(result.sender.retries);
        totals.retry_exhausted += u32::from(result.retry_exhausted);
        totals.delivered_frames += result.delivered_frames;
        totals.unique_data_packets += result.unique_data_packets;
        totals.dropped_frames += result.dropped_frames;
        totals.crc_rejects += result.crc_rejects;
        totals.half_duplex_rejects += result.half_duplex_rejects;
        totals.useful_bytes += result.useful_bytes;
        totals.on_air_bits += result.on_air_bits;
        totals.modulation_ms += result.modulation_ms;
        totals.guard_ms += result.guard_ms;
        totals.elapsed_ms += result.elapsed_ms;
        if result.success {
            totals.successful_elapsed_ms += result.elapsed_ms;
        }
    }
    Ok(totals)
}

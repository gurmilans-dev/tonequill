use super::ReliableError;
use crate::{
    config::SYMBOL_DURATION_MS,
    protocol::{framing::encode_frame, packet::Packet},
};

/// All times are integer milliseconds on a caller-provided monotonic clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub feedback_timeout_ms: u64,
    pub turnaround_ms: u64,
    pub inter_frame_guard_ms: u64,
    pub edge_padding_ms: u64,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            feedback_timeout_ms: 40_000,
            turnaround_ms: 300,
            inter_frame_guard_ms: 200,
            edge_padding_ms: 200,
        }
    }
}
impl Timing {
    pub fn validate(&self) -> Result<(), ReliableError> {
        if self.feedback_timeout_ms == 0
            || self.feedback_timeout_ms > 3_600_000
            || self.turnaround_ms > 60_000
            || self.inter_frame_guard_ms > 60_000
            || self.edge_padding_ms > 60_000
        {
            return Err(ReliableError::State(
                "timing is outside supported millisecond bounds",
            ));
        }
        Ok(())
    }
    pub fn frame_ms(&self, packet: &Packet) -> Result<u64, ReliableError> {
        let bytes = encode_frame(packet)
            .map_err(|_| ReliableError::InvalidControl("cannot serialize frame"))?;
        Ok(bytes.len() as u64 * 8 * u64::from(SYMBOL_DURATION_MS))
    }
    pub fn burst_ms(&self, packets: &[Packet]) -> Result<u64, ReliableError> {
        self.validate()?;
        if packets.is_empty() {
            return Ok(0);
        }
        let modulation = packets.iter().try_fold(0_u64, |sum, packet| {
            Ok::<_, ReliableError>(sum + self.frame_ms(packet)?)
        })?;
        Ok(modulation
            + 2 * self.edge_padding_ms
            + (packets.len() - 1) as u64 * self.inter_frame_guard_ms)
    }
}

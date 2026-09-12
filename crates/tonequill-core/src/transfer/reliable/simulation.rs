//! Deterministic whole-frame transport, separate from sample-level channel DSP.
//! The actual production sessions run against a virtual monotonic millisecond
//! clock. Serialized frame sizes determine airtime; corruptions must fail CRC.
use super::{
    ReceiveSession, ReliableError, Request, RequestKind, SendSession, SendStatistics, Status,
    StatusKind, Timing,
};
use crate::{
    protocol::{
        framing::{decode_frame, encode_frame},
        packet::{FLAG_DATA, FLAG_METADATA, FLAG_REQUEST, FLAG_STATUS, Packet},
    },
    simulation::Noise,
    transfer::{Fragmenter, MAX_FILE_SIZE, Reassembler},
};
use std::collections::{BTreeMap, VecDeque};

const MAX_QUEUED_EVENTS: usize = 4096;
const MAX_SCRIPTED_FAULTS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    OneWay,
    SelectiveRepeat { window: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Metadata,
    Data,
    Poll,
    Status,
    Complete,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultAction {
    Drop,
    Corrupt,
    Delay(u64),
    Duplicate(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptedFault {
    pub kind: FrameKind,
    pub sequence: Option<u16>,
    /// One-based occurrence among frames matching kind and optional sequence.
    pub occurrence: u32,
    pub action: FaultAction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LossModel {
    /// Applied to both metadata and data frames.
    pub data_loss: f64,
    /// Applied to requests, ordinary status, Complete and Close.
    pub control_loss: f64,
    pub corruption: f64,
    pub duplication: f64,
    pub delay_ms: u64,
    pub jitter_ms: u64,
}
impl Default for LossModel {
    fn default() -> Self {
        Self {
            data_loss: 0.0,
            control_loss: 0.0,
            corruption: 0.0,
            duplication: 0.0,
            delay_ms: 10,
            jitter_ms: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SimulationConfig {
    pub strategy: Strategy,
    pub retries: u32,
    pub seed: u64,
    pub timing: Timing,
    pub loss: LossModel,
    pub faults: Vec<ScriptedFault>,
}
impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            strategy: Strategy::SelectiveRepeat { window: 8 },
            retries: 8,
            seed: 1,
            timing: Timing::default(),
            loss: LossModel::default(),
            faults: Vec::new(),
        }
    }
}
impl SimulationConfig {
    pub fn validate(&self) -> Result<(), ReliableError> {
        self.timing.validate()?;
        for probability in [
            self.loss.data_loss,
            self.loss.control_loss,
            self.loss.corruption,
            self.loss.duplication,
        ] {
            if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
                return Err(ReliableError::State(
                    "frame loss probabilities must be finite values in 0..1",
                ));
            }
        }
        if self.retries == 0 || self.retries > 100 || self.loss.delay_ms > 3_600_000 || self.loss.jitter_ms > 3_600_000
            || self.faults.len() > MAX_SCRIPTED_FAULTS || self.faults.iter().any(|f| f.occurrence == 0 ||
                matches!(f.action, FaultAction::Delay(ms) | FaultAction::Duplicate(ms) if ms > 3_600_000)) {
            return Err(ReliableError::State("simulation resource or timing limit"));
        }
        if matches!(self.strategy, Strategy::SelectiveRepeat { window } if !(1..=32).contains(&window))
        {
            return Err(ReliableError::State("window must be 1..32"));
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SimulationResult {
    pub success: bool,
    pub receiver_committed: bool,
    pub sender_confirmed: bool,
    pub failure: Option<String>,
    pub retry_exhausted: bool,
    pub data_frames: u64,
    pub metadata_frames: u64,
    pub control_frames: u64,
    pub complete_frames: u64,
    pub dropped_frames: u64,
    pub duplicated_deliveries: u64,
    pub crc_rejects: u64,
    pub half_duplex_rejects: u64,
    pub delivered_frames: u64,
    pub unique_data_packets: u64,
    pub useful_bytes: u64,
    pub on_air_bits: u64,
    pub modulation_ms: u64,
    pub guard_ms: u64,
    pub elapsed_ms: u64,
    pub commits: u32,
    pub sender: SendStatistics,
}
impl SimulationResult {
    /// Useful original bytes from successful transfers / full simulated duration.
    pub fn goodput_bytes_per_second(&self) -> f64 {
        if self.elapsed_ms == 0 {
            0.0
        } else {
            self.useful_bytes as f64 * 1000.0 / self.elapsed_ms as f64
        }
    }
}

struct Delivery {
    from: usize,
    start_ms: u64,
    bytes: Vec<u8>,
}

struct Transport<'a> {
    config: &'a SimulationConfig,
    random: Noise,
    queue: BTreeMap<(u64, u64), Delivery>,
    serial: u64,
    faults_seen: Vec<u32>,
    /// TX plus acoustic settle intervals; source 0 is sender, source 1 receiver.
    blocked: [VecDeque<(u64, u64)>; 2],
    free_at: [u64; 2],
    result: SimulationResult,
}
impl<'a> Transport<'a> {
    fn new(config: &'a SimulationConfig) -> Self {
        Self {
            config,
            random: Noise::new(config.seed),
            queue: BTreeMap::new(),
            serial: 0,
            faults_seen: vec![0; config.faults.len()],
            blocked: Default::default(),
            free_at: [0; 2],
            result: Default::default(),
        }
    }
    fn transmit(
        &mut self,
        from: usize,
        requested_start: u64,
        packets: &[Packet],
        turning: bool,
    ) -> Result<u64, ReliableError> {
        let timing = self.config.timing;
        let turn = if turning { 2 * timing.turnaround_ms } else { 0 };
        let start = requested_start.max(self.free_at[from]) + turn;
        let end = start + timing.burst_ms(packets)?;
        self.result.guard_ms += turn
            + 2 * timing.edge_padding_ms
            + packets.len().saturating_sub(1) as u64 * timing.inter_frame_guard_ms;
        self.blocked[from].push_back((start, end + timing.turnaround_ms));
        self.free_at[from] = end + timing.turnaround_ms;
        let mut position = start + timing.edge_padding_ms;
        for packet in packets {
            let duration = timing.frame_ms(packet)?;
            self.schedule(from, position, position + duration, packet)?;
            position += duration + timing.inter_frame_guard_ms;
        }
        Ok(end)
    }
    fn schedule(
        &mut self,
        from: usize,
        start: u64,
        end: u64,
        packet: &Packet,
    ) -> Result<(), ReliableError> {
        let kind = kind(packet)?;
        match kind {
            FrameKind::Data => self.result.data_frames += 1,
            FrameKind::Metadata => self.result.metadata_frames += 1,
            _ => {
                self.result.control_frames += 1;
                if kind == FrameKind::Complete {
                    self.result.complete_frames += 1;
                }
            }
        }
        let mut bytes = encode_frame(packet)
            .map_err(|_| ReliableError::InvalidControl("simulation frame serialization"))?;
        self.result.on_air_bits += bytes.len() as u64 * 8;
        self.result.modulation_ms += end - start;
        let loss = if matches!(kind, FrameKind::Metadata | FrameKind::Data) {
            self.config.loss.data_loss
        } else {
            self.config.loss.control_loss
        };
        let mut drop_frame = self.random.uniform() < loss;
        let mut corrupt = self.random.uniform() < self.config.loss.corruption;
        let mut duplicate = (self.random.uniform() < self.config.loss.duplication).then_some(1_u64);
        let mut delay = self.config.loss.delay_ms
            + (self.random.uniform() * (self.config.loss.jitter_ms + 1) as f64) as u64;
        for (index, fault) in self.config.faults.iter().enumerate() {
            if fault.kind != kind || fault.sequence.is_some_and(|s| s != packet.sequence) {
                continue;
            }
            self.faults_seen[index] += 1;
            if self.faults_seen[index] != fault.occurrence {
                continue;
            }
            match fault.action {
                FaultAction::Drop => drop_frame = true,
                FaultAction::Corrupt => corrupt = true,
                FaultAction::Delay(ms) => delay += ms,
                FaultAction::Duplicate(ms) => duplicate = Some(ms),
            }
        }
        if drop_frame {
            self.result.dropped_frames += 1;
            return Ok(());
        }
        if corrupt {
            *bytes.last_mut().unwrap() ^= 1;
        }
        if let Some(extra) = duplicate {
            self.enqueue(
                from,
                start + delay + extra,
                end + delay + extra,
                bytes.clone(),
            )?;
            self.result.duplicated_deliveries += 1;
        }
        self.enqueue(from, start + delay, end + delay, bytes)
    }
    fn enqueue(
        &mut self,
        from: usize,
        start_ms: u64,
        end_ms: u64,
        bytes: Vec<u8>,
    ) -> Result<(), ReliableError> {
        if self.queue.len() >= MAX_QUEUED_EVENTS {
            return Err(ReliableError::State("simulation event queue limit"));
        }
        self.serial += 1;
        self.queue.insert(
            (end_ms, self.serial),
            Delivery {
                from,
                start_ms,
                bytes,
            },
        );
        Ok(())
    }
    fn next_time(&self) -> Option<u64> {
        self.queue.first_key_value().map(|(&(at, _), _)| at)
    }
    fn pop(&mut self) -> Option<(u64, usize, Packet)> {
        let ((at, _), delivery) = self.queue.pop_first()?;
        let destination = 1 - delivery.from;
        // Keep only intervals that could overlap future maximum-length frames,
        // accounting for bounded delivery delay and reordering.
        let scripted_delay: u64 = self
            .config
            .faults
            .iter()
            .map(|f| match f.action {
                FaultAction::Delay(ms) | FaultAction::Duplicate(ms) => ms,
                _ => 0,
            })
            .sum();
        let history =
            26_000 + self.config.loss.delay_ms + self.config.loss.jitter_ms + scripted_delay;
        for intervals in &mut self.blocked {
            while intervals
                .front()
                .is_some_and(|&(_, end)| end < at.saturating_sub(history))
            {
                intervals.pop_front();
            }
        }
        if self.blocked[destination]
            .iter()
            .any(|&(start, end)| delivery.start_ms < end && at > start)
        {
            self.result.half_duplex_rejects += 1;
            return None;
        }
        match decode_frame(&delivery.bytes) {
            Ok(packet) => {
                self.result.delivered_frames += 1;
                Some((at, destination, packet))
            }
            Err(_) => {
                self.result.crc_rejects += 1;
                None
            }
        }
    }
}

fn kind(packet: &Packet) -> Result<FrameKind, ReliableError> {
    Ok(match packet.flags {
        FLAG_METADATA => FrameKind::Metadata,
        FLAG_DATA => FrameKind::Data,
        FLAG_REQUEST => {
            if Request::decode(packet)?.kind == RequestKind::Close {
                FrameKind::Close
            } else {
                FrameKind::Poll
            }
        }
        FLAG_STATUS => {
            if matches!(Status::decode(packet)?.kind, StatusKind::Complete(_)) {
                FrameKind::Complete
            } else {
                FrameKind::Status
            }
        }
        _ => {
            return Err(ReliableError::InvalidControl(
                "unexpected simulation frame kind",
            ));
        }
    })
}

pub fn simulate(data: &[u8], config: &SimulationConfig) -> Result<SimulationResult, ReliableError> {
    config.validate()?;
    if data.len() as u64 > MAX_FILE_SIZE {
        return Err(crate::transfer::TransferError::FileTooLarge.into());
    }
    if config.strategy == Strategy::OneWay {
        return one_way(data, config);
    }
    let Strategy::SelectiveRepeat { window } = config.strategy else {
        unreachable!()
    };
    let mut sender = SendSession::new(0x531a0123, "sample.bin", data, window, config.retries)?;
    sender.set_timing(config.timing)?;
    let mut receiver = ReceiveSession::default();
    let mut transport = Transport::new(config);
    let end = transport.transmit(0, 0, &sender.next_burst()?, false)?;
    sender.transmitted(end + config.timing.turnaround_ms)?;
    let mut now = 0;
    let mut commits = 0;
    let mut terminal = false;
    for _ in 0..5_000_000 {
        let next_event = transport.next_time();
        let deadline = sender.deadline_ms();
        if let Some(at) = next_event.filter(|&at| deadline.is_none_or(|deadline| at <= deadline)) {
            now = at;
            let Some((_, destination, packet)) = transport.pop() else {
                continue;
            };
            if destination == 1 {
                if let Some(reply) = receiver.accept(packet, |file| {
                    if file.bytes != data {
                        return Err("incorrect verified bytes in simulator".into());
                    }
                    commits += 1;
                    Ok(())
                })? {
                    transport.transmit(1, now, &[reply], true)?;
                }
            } else {
                match sender.accept(&packet) {
                    Ok(false) | Err(ReliableError::InvalidControl(_)) => continue,
                    Err(error) => {
                        transport.result.retry_exhausted = error == ReliableError::RetryLimit;
                        transport.result.failure = Some(error.to_string());
                        terminal = true;
                        break;
                    }
                    Ok(true) => (),
                }
                if sender.is_complete() {
                    transport.result.success = true;
                    transport.result.sender_confirmed = true;
                    now = transport.transmit(0, now, &[sender.close_packet()?], true)?;
                    terminal = true;
                    break;
                }
                let end = transport.transmit(0, now, &sender.next_burst()?, true)?;
                sender.transmitted(end + config.timing.turnaround_ms)?;
            }
        } else if let Some(deadline) = deadline {
            now = deadline;
            match sender.tick(now) {
                Ok(Some(poll)) => {
                    let end = transport.transmit(0, now, &[poll], true)?;
                    sender.transmitted(end + config.timing.turnaround_ms)?;
                }
                Err(error) => {
                    transport.result.retry_exhausted = error == ReliableError::RetryLimit;
                    transport.result.failure = Some(error.to_string());
                    terminal = true;
                    break;
                }
                Ok(None) => {
                    return Err(ReliableError::State("simulation deadline made no progress"));
                }
            }
        } else {
            break;
        }
    }
    if !terminal {
        return Err(ReliableError::State(
            "simulation event budget exhausted or stalled",
        ));
    }
    transport.result.receiver_committed = receiver.is_committed();
    transport.result.unique_data_packets = u64::from(receiver.received_packets());
    transport.result.commits = commits;
    transport.result.useful_bytes = if transport.result.success {
        data.len() as u64
    } else {
        0
    };
    transport.result.elapsed_ms = now;
    transport.result.sender = sender.statistics().clone();
    Ok(transport.result)
}

fn one_way(data: &[u8], config: &SimulationConfig) -> Result<SimulationResult, ReliableError> {
    let mut transport = Transport::new(config);
    let mut receiver = Reassembler::default();
    let mut fragments = Fragmenter::new(0x531a0123, "sample.bin", data)?;
    transport.result.guard_ms = 2 * config.timing.edge_padding_ms
        + fragments.len().saturating_sub(1) as u64 * config.timing.inter_frame_guard_ms;
    let mut position = config.timing.edge_padding_ms;
    let mut end = position;
    for packet in fragments.by_ref() {
        end = position + config.timing.frame_ms(&packet)?;
        transport.schedule(0, position, end, &packet)?;
        position = end + config.timing.inter_frame_guard_ms;
        while transport.next_time().is_some_and(|at| at <= end) {
            if let Some((_, _, packet)) = transport.pop() {
                let _ = receiver.push(packet);
            }
        }
    }
    end += config.timing.edge_padding_ms;
    while let Some(at) = transport.next_time() {
        end = end.max(at);
        if let Some((_, _, packet)) = transport.pop() {
            let _ = receiver.push(packet);
        }
    }
    match receiver.finish(0x531a0123) {
        Ok(file) if file.bytes == data => {
            transport.result.success = true;
            transport.result.receiver_committed = true;
            transport.result.commits = 1;
            transport.result.useful_bytes = data.len() as u64;
        }
        Ok(_) => {
            return Err(ReliableError::State(
                "one-way baseline reconstructed wrong bytes",
            ));
        }
        Err(error) => transport.result.failure = Some(error.to_string()),
    }
    transport.result.unique_data_packets = receiver
        .progress(0x531a0123)
        .map_or(0, |p| u64::from(p.received_packets));
    transport.result.elapsed_ms = end;
    Ok(transport.result)
}

use super::{ReliableError, Request, RequestKind, Status, StatusKind, Timing, bitmap};
use crate::{
    config::MAX_PAYLOAD_SIZE,
    protocol::packet::{FLAG_DATA, FLAG_METADATA, Packet},
    transfer::{Fragmenter, Metadata},
};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SendStatistics {
    pub data_transmissions: u32,
    pub retransmitted_data: u32,
    pub metadata_transmissions: u32,
    pub polls: u32,
    pub acknowledged_packets: u32,
    pub timeouts: u32,
    pub retries: u32,
    pub feedback_rounds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderState {
    ReadyMetadata,
    WaitingMetadataAck,
    ReadyWindow,
    WaitingFeedback,
    Retransmitting,
    ReadyFinalPoll,
    WaitingComplete,
    Complete,
    Failed,
}
impl SenderState {
    pub fn is_waiting(self) -> bool {
        matches!(
            self,
            Self::WaitingMetadataAck | Self::WaitingFeedback | Self::WaitingComplete
        )
    }
}

/// One sender and one receiver take turns on the same acoustic channel. Missing
/// status triggers only another poll; missing data is resent only from a bitmap.
pub struct SendSession<'a> {
    metadata: Metadata,
    data: &'a [u8],
    window: u8,
    retry_limit: u32,
    failures: u32,
    receiver_restarts: u32,
    base: u32,
    missing: u32,
    state: SenderState,
    request: Option<Request>,
    round: u32,
    failure: Option<ReliableError>,
    timing: Timing,
    deadline_ms: Option<u64>,
    sent: Vec<bool>,
    statistics: SendStatistics,
}

impl<'a> SendSession<'a> {
    pub fn new(
        id: u32,
        filename: &str,
        data: &'a [u8],
        window: u8,
        retry_limit: u32,
    ) -> Result<Self, ReliableError> {
        if !(1..=32).contains(&window) || !(1..=100).contains(&retry_limit) {
            return Err(ReliableError::State(
                "window must be 1..32; retry limit must be 1..100",
            ));
        }
        let metadata = Fragmenter::new(id, filename, data)?.metadata().clone();
        let sent = vec![false; metadata.data_packets as usize];
        Ok(Self {
            metadata,
            data,
            window,
            retry_limit,
            failures: 0,
            receiver_restarts: 0,
            base: 0,
            missing: 0,
            state: SenderState::ReadyMetadata,
            request: None,
            round: 0,
            failure: None,
            timing: Timing::default(),
            deadline_ms: None,
            sent,
            statistics: SendStatistics::default(),
        })
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    pub fn statistics(&self) -> &SendStatistics {
        &self.statistics
    }
    pub fn is_complete(&self) -> bool {
        self.state == SenderState::Complete
    }

    pub fn state(&self) -> SenderState {
        self.state
    }
    pub fn failure(&self) -> Option<&ReliableError> {
        self.failure.as_ref()
    }
    pub fn deadline_ms(&self) -> Option<u64> {
        self.deadline_ms
    }
    pub fn set_timing(&mut self, timing: Timing) -> Result<(), ReliableError> {
        timing.validate()?;
        if self.round != 0 {
            return Err(ReliableError::State(
                "configure timing before the first burst",
            ));
        }
        self.timing = timing;
        Ok(())
    }

    /// Start the deadline only after the complete burst and TX/listen settle.
    /// Tests and simulation inject their clock; core code never sleeps.
    pub fn transmitted(&mut self, now_ms: u64) -> Result<(), ReliableError> {
        if !self.state.is_waiting() {
            return Err(ReliableError::State("no outstanding transmission"));
        }
        self.deadline_ms = Some(
            now_ms
                .checked_add(self.timing.feedback_timeout_ms)
                .ok_or(ReliableError::State("clock overflow"))?,
        );
        Ok(())
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<Option<Packet>, ReliableError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if self.deadline_ms.is_some_and(|deadline| now_ms >= deadline) {
            self.deadline_ms = None;
            return self.retry_poll().map(Some);
        }
        Ok(None)
    }

    pub fn cancel(&mut self) {
        if !self.is_complete() {
            self.set_failure(ReliableError::Cancelled);
        }
    }

    pub fn next_burst(&mut self) -> Result<Vec<Packet>, ReliableError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if self.is_complete() || self.state.is_waiting() {
            return Err(ReliableError::State(
                "no new burst while awaiting status or after success",
            ));
        }
        self.round = match self.round.checked_add(1) {
            Some(round) => round,
            None => return Err(self.set_failure(ReliableError::RetryLimit)),
        };
        let handshake = self.state == SenderState::ReadyMetadata;
        let count = if handshake {
            0
        } else {
            (self.metadata.data_packets - self.base).min(u32::from(self.window)) as u8
        };
        let request = Request {
            transfer_id: self.metadata.transfer_id,
            round: self.round,
            base: if count == 0 { 0 } else { self.base as u16 },
            count,
            kind: RequestKind::Poll,
        };
        let mut packets = Vec::new();
        if handshake {
            let mut packet = Packet::new(self.metadata.transfer_id, 0, self.metadata.encode()?);
            packet.flags = FLAG_METADATA;
            packets.push(packet);
            self.statistics.metadata_transmissions += 1;
        } else {
            for offset in 0..count {
                if self.missing & (1 << offset) == 0 {
                    continue;
                }
                let sequence = self.base + u32::from(offset);
                let start = sequence as usize * MAX_PAYLOAD_SIZE;
                let end = (start + MAX_PAYLOAD_SIZE).min(self.data.len());
                let mut packet = Packet::new(
                    self.metadata.transfer_id,
                    sequence as u16,
                    self.data[start..end].to_vec(),
                );
                packet.flags = FLAG_DATA;
                packets.push(packet);
                self.statistics.data_transmissions += 1;
                if self.sent[sequence as usize] {
                    self.statistics.retransmitted_data += 1;
                }
                self.sent[sequence as usize] = true;
            }
        }
        packets.push(request.packet()?);
        self.statistics.polls += 1;
        self.request = Some(request);
        self.state = if handshake {
            SenderState::WaitingMetadataAck
        } else if count == 0 {
            SenderState::WaitingComplete
        } else {
            SenderState::WaitingFeedback
        };
        self.deadline_ms = None;
        Ok(packets)
    }

    /// A timed-out response does not prove any data was lost. Ask again first.
    pub fn retry_poll(&mut self) -> Result<Packet, ReliableError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if !self.state.is_waiting() {
            return Err(ReliableError::State("not waiting for status"));
        }
        self.statistics.timeouts += 1;
        self.record_failure()?;
        self.statistics.polls += 1;
        self.request
            .ok_or(ReliableError::State("missing request"))?
            .packet()
    }

    /// Return false for unrelated or delayed status without changing state.
    pub fn accept(&mut self, packet: &Packet) -> Result<bool, ReliableError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if !self.state.is_waiting() {
            return Ok(false);
        }
        let status = Status::decode(packet)?;
        if Some(status.request) != self.request {
            return Ok(false);
        }
        self.statistics.feedback_rounds += 1;
        match status.kind {
            StatusKind::Complete(hash) => {
                if hash != self.metadata.sha256 {
                    return Err(self.set_failure(ReliableError::Rejected(
                        "receiver's committed SHA-256 differs",
                    )));
                }
                self.state = SenderState::Complete;
                self.statistics.acknowledged_packets = self.metadata.data_packets;
            }
            StatusKind::Failed => {
                return Err(self.set_failure(ReliableError::Rejected(
                    "receiver could not validate or commit the file",
                )));
            }
            StatusKind::Busy => {
                return Err(self.set_failure(ReliableError::Rejected(
                    "receiver is serving another transfer",
                )));
            }
            StatusKind::NeedMetadata => {
                // A peer that repeatedly forgets an acknowledged transfer must
                // not keep us alive forever by acknowledging metadata again.
                if self.state != SenderState::WaitingMetadataAck {
                    self.receiver_restarts += 1;
                    if self.receiver_restarts > self.retry_limit {
                        return Err(self.set_failure(ReliableError::RetryLimit));
                    }
                }
                self.record_failure()?;
                self.state = SenderState::ReadyMetadata;
                self.base = 0;
                self.missing = 0;
                self.statistics.acknowledged_packets = 0;
            }
            StatusKind::Receiving => {
                if self.state == SenderState::WaitingMetadataAck {
                    self.state = if self.metadata.data_packets == 0 {
                        SenderState::ReadyFinalPoll
                    } else {
                        SenderState::ReadyWindow
                    };
                    self.failures = 0;
                    self.missing =
                        bitmap(self.metadata.data_packets.min(u32::from(self.window)) as u8);
                } else {
                    let missing = bitmap(status.request.count) & !status.received;
                    if missing == 0 {
                        self.base += u32::from(status.request.count);
                        self.statistics.acknowledged_packets = self.base;
                        if status.request.count != 0 {
                            self.failures = 0;
                        }
                        self.missing = bitmap(
                            (self.metadata.data_packets - self.base).min(u32::from(self.window))
                                as u8,
                        );
                        // Complete requires a committed-hash response; an all-ones
                        // bitmap alone is insufficient, including for an empty file.
                        if self.base == self.metadata.data_packets {
                            self.record_failure()?;
                            self.state = SenderState::ReadyFinalPoll;
                        } else {
                            self.state = SenderState::ReadyWindow;
                        }
                    } else {
                        self.record_failure()?;
                        self.missing = missing;
                        self.state = SenderState::Retransmitting;
                    }
                }
            }
        }
        self.deadline_ms = None;
        Ok(true)
    }

    pub fn close_packet(&self) -> Result<Packet, ReliableError> {
        if !self.is_complete() {
            return Err(ReliableError::State("file delivery is not confirmed"));
        }
        let mut request = self
            .request
            .ok_or(ReliableError::State("missing request"))?;
        request.kind = RequestKind::Close;
        request.packet()
    }

    fn record_failure(&mut self) -> Result<(), ReliableError> {
        self.failures += 1;
        if self.failures > self.retry_limit {
            Err(self.set_failure(ReliableError::RetryLimit))
        } else {
            self.statistics.retries += 1;
            Ok(())
        }
    }

    fn set_failure(&mut self, error: ReliableError) -> ReliableError {
        self.state = SenderState::Failed;
        self.deadline_ms = None;
        self.failure = Some(error.clone());
        error
    }
}

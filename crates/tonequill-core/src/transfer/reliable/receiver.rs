use super::{ReliableError, Request, RequestKind, Status, StatusKind};
use crate::{
    protocol::packet::{FLAG_DATA, FLAG_METADATA, FLAG_REQUEST, Packet},
    transfer::{Metadata, Reassembler, ReassemblyLimits, TransferError, VerifiedFile},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverState {
    Listening,
    Receiving,
    Committed,
    Closed,
    Failed,
}

pub struct ReceiveSession {
    transfer_id: Option<u32>,
    reassembler: Reassembler,
    committed: Option<[u8; 32]>,
    failure: Option<String>,
    state: ReceiverState,
    last_complete_request: Option<Request>,
}
impl Default for ReceiveSession {
    fn default() -> Self {
        Self {
            transfer_id: None,
            reassembler: Reassembler::new(ReassemblyLimits {
                max_transfers: 1,
                ..Default::default()
            }),
            committed: None,
            failure: None,
            state: ReceiverState::Listening,
            last_complete_request: None,
        }
    }
}
impl ReceiveSession {
    pub fn transfer_id(&self) -> Option<u32> {
        self.transfer_id
    }
    pub fn is_committed(&self) -> bool {
        self.committed.is_some()
    }
    pub fn is_closed(&self) -> bool {
        self.state == ReceiverState::Closed
    }
    pub fn state(&self) -> ReceiverState {
        self.state
    }
    pub fn cancel(&mut self) {
        if !self.is_committed() {
            self.failure = Some("transfer cancelled".into());
            self.state = ReceiverState::Failed;
        }
    }
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    pub fn received_packets(&self) -> u32 {
        self.transfer_id
            .and_then(|id| self.reassembler.progress(id).ok())
            .map_or(0, |p| p.received_packets)
    }
    pub fn progress(&self) -> Option<crate::transfer::TransferProgress> {
        self.transfer_id
            .and_then(|id| self.reassembler.progress(id).ok())
    }

    /// commit is invoked exactly once, only after complete reassembly and SHA-256
    /// verification. Complete ACK means that this callback also returned success.
    /// Input must already have passed the PHY framing and CRC checks.
    pub fn accept(
        &mut self,
        packet: Packet,
        mut commit: impl FnMut(&VerifiedFile) -> Result<(), String>,
    ) -> Result<Option<Packet>, ReliableError> {
        if self.state == ReceiverState::Closed {
            return Ok(None);
        }
        if self.state == ReceiverState::Failed && packet.flags != FLAG_REQUEST {
            return Ok(None);
        }
        if packet.flags == FLAG_REQUEST {
            let request = Request::decode(&packet)?;
            if request.kind == RequestKind::Close {
                let mut poll = request;
                poll.kind = RequestKind::Poll;
                if self.last_complete_request == Some(poll) && self.committed.is_some() {
                    self.state = ReceiverState::Closed;
                }
                return Ok(None);
            }
            let mut received = 0;
            let kind = if self.transfer_id.is_some_and(|id| id != request.transfer_id) {
                StatusKind::Busy
            } else if self.failure.is_some() {
                StatusKind::Failed
            } else if self.transfer_id.is_none() {
                StatusKind::NeedMetadata
            } else {
                let expected = self
                    .reassembler
                    .progress(request.transfer_id)?
                    .expected_packets
                    .unwrap_or(0);
                if u32::from(request.base) + u32::from(request.count) > expected {
                    return Err(ReliableError::InvalidControl(
                        "requested window exceeds the transfer",
                    ));
                }
                for offset in 0..request.count {
                    if self
                        .reassembler
                        .contains_data(request.transfer_id, request.base + u16::from(offset))
                    {
                        received |= 1 << offset;
                    }
                }
                if let Some(hash) = self.committed {
                    StatusKind::Complete(hash)
                } else {
                    match self.reassembler.finish(request.transfer_id) {
                        Ok(file) => match commit(&file) {
                            Ok(()) => {
                                self.committed = Some(file.metadata.sha256);
                                self.state = ReceiverState::Committed;
                                StatusKind::Complete(file.metadata.sha256)
                            }
                            Err(error) => {
                                self.failure = Some(error);
                                self.state = ReceiverState::Failed;
                                StatusKind::Failed
                            }
                        },
                        Err(TransferError::MissingPackets { .. }) => StatusKind::Receiving,
                        Err(error) => {
                            self.failure = Some(error.to_string());
                            self.state = ReceiverState::Failed;
                            StatusKind::Failed
                        }
                    }
                }
            };
            if matches!(
                kind,
                StatusKind::Failed | StatusKind::Busy | StatusKind::NeedMetadata
            ) {
                received = 0;
            }
            if matches!(kind, StatusKind::Complete(_))
                && self
                    .last_complete_request
                    .is_none_or(|last| request.round >= last.round)
            {
                self.last_complete_request = Some(request);
            }
            return Ok(Some(
                Status {
                    request,
                    received,
                    kind,
                }
                .packet()?,
            ));
        }
        if !matches!(packet.flags, FLAG_METADATA | FLAG_DATA) {
            return Ok(None);
        }
        if self.transfer_id.is_none() {
            // Polls and data cannot claim a receiver. Metadata must validate first.
            if packet.flags != FLAG_METADATA || packet.sequence != 0 {
                return Ok(None);
            }
            Metadata::decode(packet.transfer_id, &packet.payload)?;
            self.transfer_id = Some(packet.transfer_id);
            self.state = ReceiverState::Receiving;
        }
        if self.transfer_id != Some(packet.transfer_id) {
            return Ok(None);
        }
        if let Err(error) = self.reassembler.push(packet) {
            self.failure = Some(error.to_string());
            self.state = ReceiverState::Failed;
        }
        Ok(None)
    }
}

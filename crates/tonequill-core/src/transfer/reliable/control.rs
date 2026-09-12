use super::{ReliableError, bitmap};
use crate::protocol::packet::{FLAG_REQUEST, FLAG_STATUS, PROTOCOL_VERSION, Packet};

const MAGIC: &[u8; 4] = b"SLAR";
const VERSION: u8 = 1;
const REQUEST_SIZE: usize = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Poll,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub transfer_id: u32,
    pub round: u32,
    pub base: u16,
    pub count: u8,
    pub kind: RequestKind,
}
impl Request {
    pub fn validate(&self) -> Result<(), ReliableError> {
        if self.round == 0
            || self.count > 32
            || u32::from(self.base) + u32::from(self.count) > 65536
            || (self.count == 0 && self.base != 0)
        {
            return Err(ReliableError::InvalidControl("round or window bounds"));
        }
        Ok(())
    }
    pub fn packet(&self) -> Result<Packet, ReliableError> {
        self.validate()?;
        let mut payload = MAGIC.to_vec();
        payload.push(VERSION);
        payload.push(if self.kind == RequestKind::Poll { 1 } else { 2 });
        payload.extend_from_slice(&self.round.to_be_bytes());
        payload.extend_from_slice(&self.base.to_be_bytes());
        payload.push(self.count);
        let mut packet = Packet::new(self.transfer_id, 0, payload);
        packet.flags = FLAG_REQUEST;
        Ok(packet)
    }
    pub fn decode(packet: &Packet) -> Result<Self, ReliableError> {
        validate_envelope(packet, FLAG_REQUEST, REQUEST_SIZE)?;
        if packet.payload.len() != REQUEST_SIZE {
            return Err(ReliableError::InvalidControl("request length"));
        }
        let request = Self {
            transfer_id: packet.transfer_id,
            round: u32::from_be_bytes(packet.payload[6..10].try_into().unwrap()),
            base: u16::from_be_bytes(packet.payload[10..12].try_into().unwrap()),
            count: packet.payload[12],
            kind: match packet.payload[5] {
                1 => RequestKind::Poll,
                2 => RequestKind::Close,
                _ => return Err(ReliableError::InvalidControl("request kind")),
            },
        };
        request.validate()?;
        Ok(request)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusKind {
    NeedMetadata,
    Receiving,
    /// Sent only after SHA verification AND a successful destination commit.
    Complete([u8; 32]),
    Failed,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub request: Request,
    pub received: u32,
    pub kind: StatusKind,
}
impl Status {
    pub fn packet(&self) -> Result<Packet, ReliableError> {
        self.request.validate()?;
        if self.request.kind != RequestKind::Poll
            || self.received & !bitmap(self.request.count) != 0
        {
            return Err(ReliableError::InvalidControl("status window or bitmap"));
        }
        match self.kind {
            StatusKind::NeedMetadata | StatusKind::Failed | StatusKind::Busy
                if self.received != 0 =>
            {
                return Err(ReliableError::InvalidControl(
                    "non-receipt status must have a zero bitmap",
                ));
            }
            StatusKind::Complete(_) if self.received != bitmap(self.request.count) => {
                return Err(ReliableError::InvalidControl(
                    "Complete requires every requested bit",
                ));
            }
            _ => (),
        }
        let mut packet = self.request.packet()?;
        packet.flags = FLAG_STATUS;
        packet.payload[5] = match self.kind {
            StatusKind::NeedMetadata => 1,
            StatusKind::Receiving => 2,
            StatusKind::Complete(_) => 3,
            StatusKind::Failed => 4,
            StatusKind::Busy => 5,
        };
        packet
            .payload
            .extend_from_slice(&self.received.to_be_bytes());
        if let StatusKind::Complete(hash) = self.kind {
            packet.payload.extend_from_slice(&hash);
        }
        Ok(packet)
    }
    pub fn decode(packet: &Packet) -> Result<Self, ReliableError> {
        validate_envelope(packet, FLAG_STATUS, 17)?;
        let kind = match (packet.payload[5], packet.payload.len()) {
            (1, 17) => StatusKind::NeedMetadata,
            (2, 17) => StatusKind::Receiving,
            (3, 49) => StatusKind::Complete(packet.payload[17..49].try_into().unwrap()),
            (4, 17) => StatusKind::Failed,
            (5, 17) => StatusKind::Busy,
            _ => return Err(ReliableError::InvalidControl("status kind or length")),
        };
        let status = Self {
            request: Request {
                transfer_id: packet.transfer_id,
                round: u32::from_be_bytes(packet.payload[6..10].try_into().unwrap()),
                base: u16::from_be_bytes(packet.payload[10..12].try_into().unwrap()),
                count: packet.payload[12],
                kind: RequestKind::Poll,
            },
            received: u32::from_be_bytes(packet.payload[13..17].try_into().unwrap()),
            kind,
        };
        status.packet()?; // Validate bounds before any bitmap arithmetic by callers.
        Ok(status)
    }
}

fn validate_envelope(packet: &Packet, flags: u8, minimum: usize) -> Result<(), ReliableError> {
    if packet.version != PROTOCOL_VERSION
        || packet.flags != flags
        || packet.sequence != 0
        || packet.payload.len() < minimum
        || &packet.payload[..4] != MAGIC
        || packet.payload[4] != VERSION
    {
        return Err(ReliableError::InvalidControl(
            "header, magic or schema version",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub version: u8,
    pub flags: u8,
    pub transfer_id: u32,
    pub sequence: u16,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(transfer_id: u32, sequence: u16, payload: Vec<u8>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            flags: 0,
            transfer_id,
            sequence,
            payload,
        }
    }
}
pub const PROTOCOL_VERSION: u8 = 1;

/// Exact, mutually exclusive frame kinds. Zero retains legacy raw-payload semantics.
pub const FLAG_METADATA: u8 = 0x01;
pub const FLAG_DATA: u8 = 0x02;
pub const FLAG_REQUEST: u8 = 0x04;
pub const FLAG_STATUS: u8 = 0x08;

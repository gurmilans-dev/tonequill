use thiserror::Error;

use crate::config::MAX_PAYLOAD_SIZE;

use super::packet::{
    FLAG_DATA, FLAG_METADATA, FLAG_REQUEST, FLAG_STATUS, PROTOCOL_VERSION, Packet,
};

pub const PREAMBLE: [u8; 8] = [0xAA; 8];
pub const SYNC_WORD: u16 = 0xD391;

const FIXED_HEADER_SIZE: usize = 10;
const CRC_SIZE: usize = 4;
pub const PREFIX_SIZE: usize = PREAMBLE.len() + 2 + FIXED_HEADER_SIZE;
pub const MAX_FRAME_SIZE: usize = PREFIX_SIZE + MAX_PAYLOAD_SIZE + CRC_SIZE;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u8),

    #[error("unsupported flags 0x{0:02X}")]
    UnsupportedFlags(u8),
    #[error("payload exceeds maximum supported size")]
    PayloadTooLarge,

    #[error("frame is too short")]
    FrameTooShort,

    #[error("invalid preamble")]
    InvalidPreamble,

    #[error("invalid sync word")]
    InvalidSyncWord,

    #[error("invalid payload length")]
    InvalidPayloadLength,

    #[error("CRC mismatch: expected {expected:08X}, calculated {actual:08X}")]
    CrcMismatch { expected: u32, actual: u32 },
}
pub fn encode_frame(packet: &Packet) -> Result<Vec<u8>, FrameError> {
    validate_version_flags(packet.version, packet.flags)?;
    if packet.payload.len() > MAX_PAYLOAD_SIZE {
        return Err(FrameError::PayloadTooLarge);
    }

    let payload_length = packet.payload.len() as u16;

    let mut frame = Vec::with_capacity(
        PREAMBLE.len() + 2 + FIXED_HEADER_SIZE + packet.payload.len() + CRC_SIZE,
    );

    // Physical framing
    frame.extend_from_slice(&PREAMBLE);
    frame.extend_from_slice(&SYNC_WORD.to_be_bytes());

    // Il CRC comincia da VERSION
    let crc_start = frame.len();

    frame.push(packet.version);
    frame.push(packet.flags);

    frame.extend_from_slice(&packet.transfer_id.to_be_bytes());

    frame.extend_from_slice(&packet.sequence.to_be_bytes());

    frame.extend_from_slice(&payload_length.to_be_bytes());

    frame.extend_from_slice(&packet.payload);

    let crc = crc32fast::hash(&frame[crc_start..]);

    frame.extend_from_slice(&crc.to_be_bytes());

    Ok(frame)
}
pub fn decode_frame(frame: &[u8]) -> Result<Packet, FrameError> {
    let minimum_size = PREAMBLE.len() + 2 + FIXED_HEADER_SIZE + CRC_SIZE;

    if frame.len() < minimum_size {
        return Err(FrameError::FrameTooShort);
    }

    // PREAMBLE
    if frame[..PREAMBLE.len()] != PREAMBLE {
        return Err(FrameError::InvalidPreamble);
    }

    let mut cursor = PREAMBLE.len();

    // SYNC WORD
    let sync = u16::from_be_bytes([frame[cursor], frame[cursor + 1]]);

    if sync != SYNC_WORD {
        return Err(FrameError::InvalidSyncWord);
    }

    cursor += 2;

    let crc_start = cursor;

    // VERSION
    let version = frame[cursor];
    cursor += 1;

    // FLAGS
    let flags = frame[cursor];
    cursor += 1;

    // TRANSFER ID
    let transfer_id = u32::from_be_bytes([
        frame[cursor],
        frame[cursor + 1],
        frame[cursor + 2],
        frame[cursor + 3],
    ]);

    cursor += 4;

    // SEQUENCE
    let sequence = u16::from_be_bytes([frame[cursor], frame[cursor + 1]]);

    cursor += 2;

    // PAYLOAD LENGTH
    let payload_length = u16::from_be_bytes([frame[cursor], frame[cursor + 1]]) as usize;

    cursor += 2;

    if payload_length > MAX_PAYLOAD_SIZE {
        return Err(FrameError::InvalidPayloadLength);
    }

    let expected_total = PREAMBLE.len() + 2 + FIXED_HEADER_SIZE + payload_length + CRC_SIZE;

    if frame.len() != expected_total {
        return Err(FrameError::InvalidPayloadLength);
    }

    let payload_end = cursor + payload_length;

    let payload = frame[cursor..payload_end].to_vec();

    cursor = payload_end;

    // CRC ricevuto
    let expected_crc = u32::from_be_bytes([
        frame[cursor],
        frame[cursor + 1],
        frame[cursor + 2],
        frame[cursor + 3],
    ]);

    // CRC calcolato
    let actual_crc = crc32fast::hash(&frame[crc_start..cursor]);

    if expected_crc != actual_crc {
        return Err(FrameError::CrcMismatch {
            expected: expected_crc,
            actual: actual_crc,
        });
    }

    validate_version_flags(version, flags)?;

    Ok(Packet {
        version,
        flags,
        transfer_id,
        sequence,
        payload,
    })
}

pub fn frame_size_from_prefix(frame: &[u8]) -> Result<usize, FrameError> {
    const PAYLOAD_LENGTH_OFFSET: usize = 8  // preamble
        + 2 // sync
        + 1 // version
        + 1 // flags
        + 4 // transfer id
        + 2; // sequence

    if frame.len() < PREFIX_SIZE {
        return Err(FrameError::FrameTooShort);
    }

    if frame[..PREAMBLE.len()] != PREAMBLE {
        return Err(FrameError::InvalidPreamble);
    }

    let sync = u16::from_be_bytes([frame[PREAMBLE.len()], frame[PREAMBLE.len() + 1]]);

    if sync != SYNC_WORD {
        return Err(FrameError::InvalidSyncWord);
    }

    validate_version_flags(frame[PREAMBLE.len() + 2], frame[PREAMBLE.len() + 3])?;

    let payload_length = u16::from_be_bytes([
        frame[PAYLOAD_LENGTH_OFFSET],
        frame[PAYLOAD_LENGTH_OFFSET + 1],
    ]) as usize;

    if payload_length > MAX_PAYLOAD_SIZE {
        return Err(FrameError::InvalidPayloadLength);
    }

    Ok(PREFIX_SIZE + payload_length + CRC_SIZE)
}

pub(crate) fn validate_version_flags(version: u8, flags: u8) -> Result<(), FrameError> {
    if version != PROTOCOL_VERSION {
        return Err(FrameError::UnsupportedVersion(version));
    }
    // These are frame kinds, not combinable feature bits. Unknown combinations fail closed.
    if !matches!(
        flags,
        0 | FLAG_METADATA | FLAG_DATA | FLAG_REQUEST | FLAG_STATUS
    ) {
        return Err(FrameError::UnsupportedFlags(flags));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let original = Packet::new(0x12345678, 42, b"Tonequill".to_vec());

        let encoded = encode_frame(&original).unwrap();

        let decoded = decode_frame(&encoded).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn detects_corrupted_payload() {
        let packet = Packet::new(0x12345678, 1, b"hello world".to_vec());

        let mut encoded = encode_frame(&packet).unwrap();

        // Corrompiamo intenzionalmente
        // un byte del payload.
        let payload_position = PREAMBLE.len() + 2 + FIXED_HEADER_SIZE;

        encoded[payload_position] ^= 0x01;

        let result = decode_frame(&encoded);

        assert!(matches!(result, Err(FrameError::CrcMismatch { .. })));
    }
}

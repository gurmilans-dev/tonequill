use tonequill_core::protocol::{
    framing::{FrameError, PREFIX_SIZE, decode_frame, encode_frame, frame_size_from_prefix},
    packet::Packet,
};

#[test]
fn every_protected_single_bit_mutation_is_rejected() {
    let frame = encode_frame(&Packet::new(3, 7, vec![0xa5; 32])).unwrap();
    for index in 10..frame.len() {
        for bit in 0..8 {
            let mut corrupted = frame.clone();
            corrupted[index] ^= 1 << bit;
            assert!(decode_frame(&corrupted).is_err(), "byte={index}, bit={bit}");
        }
    }
}

#[test]
fn validates_reserved_fields_even_with_valid_crc() {
    for (offset, value, error) in [
        (10, 2, FrameError::UnsupportedVersion(2)),
        (11, 3, FrameError::UnsupportedFlags(3)),
        (11, 16, FrameError::UnsupportedFlags(16)),
        (11, 255, FrameError::UnsupportedFlags(255)),
    ] {
        let mut frame = encode_frame(&Packet::new(3, 7, vec![0xa5; 32])).unwrap();
        frame[offset] = value;
        let end = frame.len() - 4;
        let crc = crc32fast::hash(&frame[10..end]);
        frame[end..].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(decode_frame(&frame), Err(error));
        assert!(frame_size_from_prefix(&frame[..PREFIX_SIZE]).is_err());
    }
}

#[test]
fn short_prefixes_and_incorrect_lengths_never_panic() {
    let frame = encode_frame(&Packet::new(3, 7, vec![0xa5; 32])).unwrap();
    for length in 0..frame.len() {
        assert!(decode_frame(&frame[..length]).is_err());
        if length < PREFIX_SIZE {
            assert!(frame_size_from_prefix(&frame[..length]).is_err());
        }
    }
    let mut extra = frame.clone();
    extra.push(0);
    assert_eq!(decode_frame(&extra), Err(FrameError::InvalidPayloadLength));
}

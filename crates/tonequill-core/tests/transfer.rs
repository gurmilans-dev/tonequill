use sha2::{Digest, Sha256};
use tonequill_core::{
    config::MAX_PAYLOAD_SIZE,
    protocol::{
        framing::{FrameError, PREFIX_SIZE, decode_frame, encode_frame},
        packet::{FLAG_DATA, FLAG_METADATA, Packet},
    },
    transfer::{
        Fragmenter, InsertOutcome, MAX_DATA_PACKETS, MAX_FILE_SIZE, Metadata, Reassembler,
        ReassemblyLimits, TransferError,
        metadata::{MAX_FILENAME_BYTES, data_packet_count},
        safe_filename,
        waveform::TransmissionPlan,
    },
};

fn bytes(size: usize) -> Vec<u8> {
    let mut state = 0xcafe_7351_u32;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}
fn packets(data: &[u8]) -> Vec<Packet> {
    Fragmenter::new(0x12345678, "sample.bin", data)
        .unwrap()
        .collect()
}
fn add(receiver: &mut Reassembler, packet: &Packet) -> Result<InsertOutcome, TransferError> {
    // Exercise real byte framing and CRC, not only constructed Packet objects.
    receiver.push(decode_frame(&encode_frame(packet).unwrap()).unwrap())
}

#[test]
fn deterministic_fragmentation_and_binary_reassembly_boundaries() {
    for size in [
        0,
        1,
        255,
        MAX_PAYLOAD_SIZE,
        MAX_PAYLOAD_SIZE + 1,
        1024,
        10 * 1024,
    ] {
        let data = bytes(size);
        let fragments = packets(&data);
        assert_eq!(fragments, packets(&data));
        assert_eq!(fragments.len(), 1 + size.div_ceil(MAX_PAYLOAD_SIZE));
        assert_eq!(fragments[0].flags, FLAG_METADATA);
        assert_eq!(fragments[0].sequence, 0);
        let mut receiver = Reassembler::default();
        for (index, packet) in fragments.iter().enumerate() {
            assert!(packet.payload.len() <= MAX_PAYLOAD_SIZE);
            if index > 0 {
                assert_eq!(packet.flags, FLAG_DATA);
                assert_eq!(packet.sequence as usize, index - 1);
            }
            add(&mut receiver, packet).unwrap();
        }
        let file = receiver.finish(0x12345678).unwrap();
        assert_eq!(file.bytes, data, "size={size}");
        assert_eq!(
            file.metadata.sha256,
            <[u8; 32]>::from(Sha256::digest(&data))
        );
        assert!(receiver.missing_sequences(0x12345678).unwrap().is_empty());
    }
    assert!(std::str::from_utf8(&bytes(1024)).is_err());
}

#[test]
fn missing_packets_and_metadata_never_complete() {
    let fragments = packets(&bytes(1024));
    let mut receiver = Reassembler::default();
    for (index, packet) in fragments.iter().enumerate() {
        if index != 2 {
            add(&mut receiver, packet).unwrap();
        }
    }
    assert_eq!(receiver.missing_sequences(0x12345678).unwrap(), [1]);
    assert_eq!(
        receiver.finish(0x12345678).unwrap_err(),
        TransferError::MissingPackets {
            missing: 1,
            received: 3,
            expected: 4
        }
    );
    let mut receiver = Reassembler::default();
    for packet in &fragments[1..] {
        add(&mut receiver, packet).unwrap();
    }
    assert_eq!(
        receiver.finish(0x12345678).unwrap_err(),
        TransferError::MissingMetadata
    );
    assert_eq!(
        receiver.progress(0x12345678).unwrap().expected_packets,
        None
    );
}

#[test]
fn reverse_order_and_identical_duplicates_including_late_metadata() {
    let data = bytes(10 * 1024 + 19);
    let fragments = packets(&data);
    let mut receiver = Reassembler::default();
    for packet in fragments.iter().rev() {
        assert_ne!(
            add(&mut receiver, packet).unwrap(),
            InsertOutcome::Duplicate
        );
        assert_eq!(
            add(&mut receiver, packet).unwrap(),
            InsertOutcome::Duplicate
        );
    }
    let progress = receiver.progress(0x12345678).unwrap();
    assert_eq!(progress.duplicate_frames, fragments.len());
    assert_eq!(progress.received_packets as usize, fragments.len() - 1);
    assert_eq!(receiver.finish(0x12345678).unwrap().bytes, data);
}

#[test]
fn conflicting_duplicates_and_metadata_poison_even_previously_complete_transfer() {
    let fragments = packets(&bytes(257));
    for conflict_metadata in [false, true] {
        let mut receiver = Reassembler::default();
        for packet in &fragments {
            add(&mut receiver, packet).unwrap();
        }
        assert!(receiver.finish(0x12345678).is_ok());
        let mut conflict = fragments[if conflict_metadata { 0 } else { 1 }].clone();
        if conflict_metadata {
            conflict.payload[17] ^= 1;
        } else {
            conflict.payload[0] ^= 1;
        }
        let expected = if conflict_metadata {
            TransferError::ConflictingMetadata
        } else {
            TransferError::ConflictingDuplicate(0)
        };
        assert_eq!(add(&mut receiver, &conflict), Err(expected.clone()));
        assert_eq!(add(&mut receiver, &fragments[0]), Err(expected.clone()));
        assert_eq!(receiver.finish(0x12345678).unwrap_err(), expected);
    }
}

#[test]
fn final_sha256_is_checked_independently_of_packet_crc() {
    for change_hash in [false, true] {
        let mut fragments = packets(&bytes(257));
        if change_hash {
            fragments[0].payload[17] ^= 1;
        } else {
            fragments[2].payload[0] ^= 1;
        }
        let mut receiver = Reassembler::default();
        for packet in &fragments {
            add(&mut receiver, packet).unwrap();
        }
        assert_eq!(
            receiver.finish(0x12345678).unwrap_err(),
            TransferError::HashMismatch
        );
    }
}

#[test]
fn transfer_frame_payload_and_headers_are_crc_protected() {
    for packet in packets(&bytes(257)) {
        let original = encode_frame(&packet).unwrap();
        for index in 10..original.len() {
            let mut corrupted = original.clone();
            corrupted[index] ^= 1;
            assert!(decode_frame(&corrupted).is_err());
        }
        let mut corrupted = original;
        corrupted[PREFIX_SIZE] ^= 1;
        assert!(matches!(
            decode_frame(&corrupted),
            Err(FrameError::CrcMismatch { .. })
        ));
    }
}

#[test]
fn bad_packet_count_size_sequence_and_empty_data_fail_closed() {
    for variant in 0..7 {
        for metadata_first in [false, true] {
            let mut fragments = packets(&bytes(257));
            match variant {
                0 => fragments[0].payload[16] = 3, // inconsistent count
                1 => fragments[0].sequence = 1,
                2 => fragments[2].sequence = 2,    // out of range
                3 => fragments[2].payload.push(0), // wrong last length
                4 => fragments[1].payload.pop().map(|_| ()).unwrap(),
                5 => fragments[1].payload.clear(),
                _ => fragments[0].payload[12] = 0, // size now 256, count still 2
            }
            if !metadata_first {
                fragments.reverse();
            }
            let mut receiver = Reassembler::default();
            let mut rejected = false;
            for packet in &fragments {
                rejected |= add(&mut receiver, packet).is_err();
            }
            assert!(rejected, "variant={variant}");
            assert!(receiver.finish(0x12345678).is_err());
        }
    }
}

#[test]
fn metadata_parser_rejects_truncation_extensions_versions_and_unsafe_names() {
    let meta = Metadata::decode(0x12345678, &packets(&[])[0].payload).unwrap();
    let encoded = meta.encode().unwrap();
    for length in 0..encoded.len() {
        assert!(Metadata::decode(meta.transfer_id, &encoded[..length]).is_err());
    }
    for index in [0, 4, 49] {
        let mut bad = encoded.clone();
        bad[index] ^= 0x80;
        assert!(Metadata::decode(meta.transfer_id, &bad).is_err());
    }
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(Metadata::decode(meta.transfer_id, &trailing).is_err());
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "a/b",
        "a\\b",
        "C:evil",
        "/absolute",
        "x\0",
        "x\n",
        "café",
        "CON.txt",
        "lpt1",
        "COM9.bin",
        "file.",
    ] {
        let mut bad = encoded[..50].to_vec();
        bad[49] = name.len() as u8;
        bad.extend_from_slice(name.as_bytes());
        assert!(
            Metadata::decode(meta.transfer_id, &bad).is_err(),
            "name={name:?}"
        );
        let mut safe = meta.clone();
        safe.filename = safe_filename(name);
        assert!(safe.validate().is_ok());
    }
    let mut long = meta;
    long.filename = "x".repeat(MAX_FILENAME_BYTES + 1);
    assert!(long.encode().is_err());
    assert_eq!(safe_filename(&"x".repeat(500)).len(), MAX_FILENAME_BYTES);
}

#[test]
fn metadata_wire_layout_matches_a_known_sha256_vector() {
    // SHA-256("abc"), a 3-byte file and a 5-byte ASCII filename. This fixed
    // fixture independently fixes all offsets, field widths and byte order.
    let payload = Fragmenter::new(0x12345678, "a.bin", b"abc")
        .unwrap()
        .next()
        .unwrap()
        .payload;
    let expected_hex = concat!(
        "534c5446",
        "01",
        "0000000000000003",
        "00000001",
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "05",
        "612e62696e"
    );
    let expected: Vec<u8> = (0..expected_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&expected_hex[i..i + 2], 16).unwrap())
        .collect();
    assert_eq!(payload, expected);
    let mut invalid = Metadata::decode(1, &payload).unwrap();
    invalid.data_packets = 2;
    assert_eq!(invalid.payload_size(1), Err(TransferError::SizeMismatch));
}

#[test]
fn sequence_capacity_and_size_limits_do_not_wrap() {
    assert_eq!(data_packet_count(MAX_FILE_SIZE).unwrap(), MAX_DATA_PACKETS);
    assert_eq!(
        data_packet_count(MAX_FILE_SIZE + 1),
        Err(TransferError::FileTooLarge)
    );
    assert_eq!(
        data_packet_count(u64::MAX),
        Err(TransferError::FileTooLarge)
    );
    // Packet iteration is cheap even at the wire limit: no waveform, no retained packets.
    let data = vec![0x93; MAX_FILE_SIZE as usize];
    let mut fragments = Fragmenter::new(1, "max.bin", &data).unwrap();
    assert_eq!(fragments.len(), 65537);
    assert_eq!(fragments.nth(65536).unwrap().sequence, u16::MAX);
    assert_eq!(fragments.len(), 0);
    assert!(fragments.next().is_none());
}

#[test]
fn interleaved_transfer_ids_are_isolated() {
    let a = bytes(1024);
    let b = bytes(513);
    let pa: Vec<_> = Fragmenter::new(1, "a.bin", &a).unwrap().collect();
    let pb: Vec<_> = Fragmenter::new(2, "b.bin", &b).unwrap().collect();
    let mut receiver = Reassembler::default();
    for i in 0..pa.len().max(pb.len()) {
        if let Some(packet) = pb.get(i) {
            add(&mut receiver, packet).unwrap();
        }
        if let Some(packet) = pa.get(i) {
            add(&mut receiver, packet).unwrap();
        }
    }
    assert_eq!(receiver.transfer_ids().collect::<Vec<_>>(), [1, 2]);
    assert_eq!(receiver.finish(1).unwrap().bytes, a);
    assert_eq!(receiver.finish(2).unwrap().bytes, b);
}

#[test]
fn reassembly_limits_apply_before_allocating_and_duplicates_are_free() {
    let fragments = packets(&bytes(257));
    for limits in [
        ReassemblyLimits {
            max_transfers: 1,
            max_buffered_bytes: 256,
            max_buffered_packets: 2,
        },
        ReassemblyLimits {
            max_transfers: 1,
            max_buffered_bytes: 1024,
            max_buffered_packets: 1,
        },
    ] {
        let mut receiver = Reassembler::new(limits);
        add(&mut receiver, &fragments[1]).unwrap();
        assert_eq!(
            add(&mut receiver, &fragments[1]).unwrap(),
            InsertOutcome::Duplicate
        );
        assert!(matches!(
            add(&mut receiver, &fragments[2]),
            Err(TransferError::ResourceLimit(_))
        ));
        let mut other = fragments[0].clone();
        other.transfer_id = 42;
        assert!(matches!(
            add(&mut receiver, &other),
            Err(TransferError::ResourceLimit(_))
        ));
    }
}

#[test]
fn overhead_is_calculated_from_the_actual_wire_representation() {
    for (size, count, frame_bytes, bits, total_samples) in [
        (1024, 4, 1204, 9632, 4_680_960),
        (10240, 40, 11284, 90272, 43_733_760),
    ] {
        let data = bytes(size);
        let fragments = Fragmenter::new(1, "sample.bin", &data).unwrap();
        let plan = TransmissionPlan::new(fragments.metadata()).unwrap();
        assert_eq!(plan.data_packets, count);
        assert_eq!(plan.control_frames, 1);
        assert_eq!(plan.frame_bytes, frame_bytes);
        assert_eq!(plan.modulated_bits, bits);
        assert_eq!(plan.total_samples, total_samples);
        assert_eq!(
            fragments
                .map(|p| encode_frame(&p).unwrap().len() as u64)
                .sum::<u64>(),
            frame_bytes
        );
    }
}

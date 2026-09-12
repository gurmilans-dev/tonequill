use tonequill_core::{
    config::SAMPLE_RATE,
    modulation::bfsk::modulate,
    protocol::{
        bitstream::bytes_to_bits,
        framing::{PREFIX_SIZE, encode_frame},
        packet::Packet,
    },
    receiver::{FrameScanner, ReceiverOptions, SCAN_STRIDE_SAMPLES, scan},
    simulation::{Channel, Noise},
    transfer::{
        Fragmenter, Reassembler,
        waveform::{EDGE_PADDING_SAMPLES, INTER_FRAME_SAMPLES, frame_samples},
    },
};

#[test]
fn multi_frame_transfer_with_padding_silence_noise_reordering_and_duplicates() {
    let data: Vec<u8> = (0..513).map(|i| (i * 73 + 9) as u8).collect();
    let fragments: Vec<_> = Fragmenter::new(7, "binary.bin", &data).unwrap().collect();
    let mut signal = vec![0.0; EDGE_PADDING_SAMPLES + 137];
    let mut noise = Noise::new(71);
    // Metadata can arrive late; duplicates are real acoustic repetitions.
    for index in [3, 1, 0, 1, 2, 0] {
        signal.extend(frame_samples(&fragments[index]).unwrap());
        signal.extend((0..INTER_FRAME_SAMPLES).map(|_| (noise.gaussian() * 0.03) as f32));
        signal.extend(vec![0.0; 733]);
    }
    signal.extend(vec![0.0; EDGE_PADDING_SAMPLES]);
    let mut receiver = Reassembler::default();
    let mut scanner = FrameScanner::default();
    let mut attempts = Vec::new();
    // Deliberately unrelated to symbols, acquisition hops or scanner boundaries.
    for chunk in signal.chunks(8191) {
        attempts.extend(scanner.push(chunk).unwrap());
    }
    attempts.extend(scanner.finish().unwrap());
    assert_eq!(attempts.len(), 6);
    for attempt in attempts {
        receiver.push(attempt.result.unwrap()).unwrap();
    }
    assert_eq!(receiver.progress(7).unwrap().duplicate_frames, 2);
    assert_eq!(receiver.finish(7).unwrap().bytes, data);
}

#[test]
fn a_corrupt_header_cannot_skip_a_later_valid_frame() {
    let packet = Packet::new(9, 0, b"short".to_vec());
    let mut corrupt = encode_frame(&packet).unwrap();
    // Plausible but false maximum length would cover the following real frame.
    corrupt[PREFIX_SIZE - 2..PREFIX_SIZE].copy_from_slice(&256_u16.to_be_bytes());
    let mut signal = modulate(&bytes_to_bits(&corrupt));
    signal.extend(vec![0.0; INTER_FRAME_SAMPLES]);
    signal.extend(frame_samples(&packet).unwrap());
    let attempts = scan(&signal, ReceiverOptions::default()).unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(attempts[0].result.is_err());
    assert_eq!(attempts[1].result.as_ref().unwrap(), &packet);
}

#[test]
fn crc_failure_followed_by_valid_repetition_recovers_transfer() {
    let fragments: Vec<_> = Fragmenter::new(5, "ok.bin", b"crc guarded")
        .unwrap()
        .collect();
    let mut bad = encode_frame(&fragments[1]).unwrap();
    bad[PREFIX_SIZE] ^= 1;
    let mut signal = frame_samples(&fragments[0]).unwrap();
    signal.extend(vec![0.0; INTER_FRAME_SAMPLES]);
    signal.extend(modulate(&bytes_to_bits(&bad)));
    signal.extend(vec![0.0; INTER_FRAME_SAMPLES]);
    signal.extend(frame_samples(&fragments[1]).unwrap());
    let attempts = scan(&signal, ReceiverOptions::default()).unwrap();
    assert_eq!(attempts.len(), 3);
    assert!(attempts[1].result.is_err());
    let mut receiver = Reassembler::default();
    for attempt in attempts {
        if let Ok(packet) = attempt.result {
            receiver.push(packet).unwrap();
        }
    }
    assert_eq!(receiver.finish(5).unwrap().bytes, b"crc guarded");
}

#[test]
fn scan_window_boundaries_do_not_lose_or_duplicate_maximum_frames() {
    // One frame begins just before a window boundary, one at a later boundary,
    // one just after. Clock stretch makes the 22.4-second frames longer as well.
    for delta in [-241_i64, 0, 241] {
        let packet = Packet::new(6, 17, vec![0xff; 256]);
        let start = (SCAN_STRIDE_SAMPLES as i64 + delta) as usize;
        let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
        let mut signal = vec![0.0; start];
        signal.extend(
            Channel {
                clock_ppm: 5000.0,
                phase_radians: 1.3,
                ..Channel::default()
            }
            .transmit(&bits)
            .unwrap(),
        );
        signal.resize(3 * SCAN_STRIDE_SAMPLES + 211, 0.0);
        let attempts = scan(&signal, ReceiverOptions::default()).unwrap();
        assert_eq!(attempts.len(), 1, "delta={delta}");
        assert_eq!(attempts[0].result.as_ref().unwrap(), &packet);
        assert!((attempts[0].acquisition.start_sample - start as f64).abs() < 240.0);
    }
}

#[test]
fn nested_frame_bytes_in_a_valid_payload_are_not_independent_packets() {
    let nested = Packet::new(2, 0, b"inner".to_vec());
    let mut payload = vec![0; 32];
    payload.extend(encode_frame(&nested).unwrap());
    payload.extend(vec![0; 20]);
    let outer = Packet::new(1, 0, payload);
    let signal = frame_samples(&outer).unwrap();
    let attempts = scan(&signal, ReceiverOptions::default()).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].result.as_ref().unwrap(), &outer);
}

#[test]
fn silence_empty_input_and_nonfinite_chunks() {
    assert!(scan(&[], ReceiverOptions::default()).unwrap().is_empty());
    assert!(
        scan(&vec![0.0; SAMPLE_RATE as usize], ReceiverOptions::default())
            .unwrap()
            .is_empty()
    );
    assert!(FrameScanner::default().push(&[f32::NAN]).is_err());
}

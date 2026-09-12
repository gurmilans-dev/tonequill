use tonequill_core::{
    config::SAMPLE_RATE,
    protocol::{
        bitstream::bytes_to_bits,
        framing::{PREFIX_SIZE, encode_frame},
        packet::Packet,
    },
    receiver::{LiveDecoder, ReceiverOptions, inspect},
    simulation::{Channel, Echo, Noise},
    transfer::waveform::frame_samples,
};

#[test]
fn live_frames_arrive_without_eof_or_a_full_offline_window() {
    let mut decoder = LiveDecoder::default();
    let mut seen = Vec::new();
    for i in 0..5 {
        let packet = Packet::new(7, i, vec![i as u8; 17]);
        let mut samples = vec![0.0; 9600];
        samples.extend(frame_samples(&packet).unwrap());
        samples.extend(vec![0.0; 16800]); // At most 350 ms release latency, no EOF.
        for chunk in samples.chunks(997) {
            seen.extend(
                decoder
                    .push(chunk)
                    .unwrap()
                    .into_iter()
                    .filter_map(|a| a.result.ok()),
            );
        }
        assert_eq!(seen.last(), Some(&packet));
        assert_eq!(seen.len(), i as usize + 1);
    }
}

#[test]
fn incremental_trimming_preserves_the_offline_acquisition_grid_in_multipath() {
    let mut rng = Noise::new(42);
    let packet = Packet::new(
        0x9571ba32,
        3,
        (0..59).map(|_| (rng.uniform() * 256.0) as u8).collect(),
    );
    let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
    let waveform = Channel {
        seed: 42,
        leading_samples: 6037,
        trailing_samples: 24000,
        gain: 0.05,
        one_carrier_start_gain: 0.2,
        one_carrier_gain: 0.4,
        carrier_ramp_symbols: 160,
        snr_db: Some(20.0),
        phase_radians: 1.3,
        echoes: vec![
            Echo {
                delay_samples: 667,
                gain: 0.8,
                end_gain_ratio: 0.8,
            },
            Echo {
                delay_samples: 960,
                gain: 0.3,
                end_gain_ratio: 1.1,
            },
        ],
        ..Channel::default()
    }
    .transmit(&bits)
    .unwrap();
    for padding in [0, 137, 12003] {
        let mut samples = vec![0.0; padding];
        samples.extend_from_slice(&waveform);
        let offline = inspect(&samples, ReceiverOptions::default()).unwrap();
        let expected = offline
            .iter()
            .find(|a| a.result.as_ref() == Ok(&packet))
            .unwrap();
        let mut decoder = LiveDecoder::default();
        let mut incremental = Vec::new();
        for chunk in samples.chunks(997) {
            incremental.extend(decoder.push(chunk).unwrap());
        }
        // No flush/EOF: a complete frame must be delivered while listening.
        assert_eq!(incremental.len(), 1, "padding={padding}: {incremental:?}");
        let actual = &incremental[0];
        assert_eq!(actual.result.as_ref(), Ok(&packet), "padding={padding}");
        assert_eq!(actual.bits(), bits);
        assert!(
            (actual.acquisition.start_sample - expected.acquisition.start_sample).abs() < 1e-6,
            "padding={padding}: live={}, offline={}",
            actual.acquisition.start_sample,
            expected.acquisition.start_sample
        );
    }
}

#[test]
fn maximum_clock_stretched_frame_survives_incremental_buffers() {
    let packet = Packet::new(3, 0, vec![0xe7; 256]);
    let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
    let samples = Channel {
        clock_ppm: 5000.0,
        leading_samples: 3351,
        trailing_samples: 24000,
        phase_radians: 0.77,
        ..Default::default()
    }
    .transmit(&bits)
    .unwrap();
    let mut decoder = LiveDecoder::default();
    let mut packets = Vec::new();
    for chunk in samples.chunks(4801) {
        packets.extend(
            decoder
                .push(chunk)
                .unwrap()
                .into_iter()
                .filter_map(|a| a.result.ok()),
        );
    }
    assert_eq!(packets, [packet]);
}

#[test]
fn reset_discards_partial_audio_but_next_frame_is_recovered() {
    let packet = Packet::new(3, 0, b"restart".to_vec());
    let samples = frame_samples(&packet).unwrap();
    let mut decoder = LiveDecoder::default();
    decoder.push(&samples[..50000]).unwrap();
    decoder.reset();
    let mut tail = samples;
    tail.extend(vec![0.0; SAMPLE_RATE as usize / 2]);
    let packets: Vec<_> = decoder
        .push(&tail)
        .unwrap()
        .into_iter()
        .filter_map(|a| a.result.ok())
        .collect();
    assert_eq!(packets, [packet]);
}

#[test]
fn corrupt_long_candidate_does_not_permanently_hide_a_short_packet() {
    let packet = Packet::new(3, 0, b"later".to_vec());
    let mut frame = encode_frame(&packet).unwrap();
    frame[PREFIX_SIZE - 2..PREFIX_SIZE].copy_from_slice(&256_u16.to_be_bytes());
    let mut samples = tonequill_core::modulation::bfsk::modulate(&bytes_to_bits(&frame));
    samples.extend(vec![0.0; 9600]);
    samples.extend(frame_samples(&packet).unwrap());
    samples.resize(26 * SAMPLE_RATE as usize, 0.0);
    let mut decoder = LiveDecoder::default();
    let attempts = decoder.push(&samples).unwrap();
    assert!(attempts.iter().any(|a| a.result.is_err()));
    assert_eq!(
        attempts
            .into_iter()
            .filter_map(|a| a.result.ok())
            .collect::<Vec<_>>(),
        [packet]
    );
}

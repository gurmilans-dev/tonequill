use tonequill_core::{
    config::MAX_PAYLOAD_SIZE,
    modulation::bfsk::modulate,
    protocol::{
        bitstream::bytes_to_bits,
        framing::{FrameError, PREFIX_SIZE, encode_frame},
        packet::Packet,
    },
    receiver::{ReceiveError, ReceiverOptions, compare_bits, inspect, receive},
    simulation::{Channel, Noise},
};

fn packet(length: usize) -> Packet {
    Packet::new(
        0xcafe_0123,
        17,
        (0..length).map(|i| (i * 73 + 19) as u8).collect(),
    )
}
fn assert_channel(packet: &Packet, channel: Channel) {
    let bits = bytes_to_bits(&encode_frame(packet).unwrap());
    let samples = channel.transmit(&bits).unwrap();
    let attempts = inspect(&samples, ReceiverOptions::default())
        .unwrap_or_else(|e| panic!("{channel:?}: {e}"));
    let attempt = attempts
        .iter()
        .find(|a| a.result.is_ok())
        .unwrap_or(&attempts[0]);
    assert_eq!(
        attempt
            .result
            .as_ref()
            .unwrap_or_else(|e| panic!("{channel:?}: {e}; {:?}", attempt.acquisition)),
        packet
    );
    let comparison = compare_bits(&bits, &attempt.bits());
    assert_eq!(comparison.bit_errors, 0);
    assert_eq!(comparison.missing_bits, 0);
}

#[test]
fn production_transmitter_roundtrips_boundary_payload_sizes() {
    for length in [0, 1, 2, 17, 128, 255, 256] {
        let packet = packet(length);
        let signal = modulate(&bytes_to_bits(&encode_frame(&packet).unwrap()));
        assert_eq!(receive(&signal).unwrap(), packet, "length={length}");
    }
}

#[test]
fn arbitrary_alignment_phase_gain_and_dc() {
    for (i, offset) in [0, 1, 23, 119, 239, 479, 3457, 96013]
        .into_iter()
        .enumerate()
    {
        assert_channel(
            &packet(17),
            Channel {
                leading_samples: offset,
                trailing_samples: 157,
                gain: [0.0005, 0.01, 0.7, 1.5][i % 4],
                phase_radians: i as f64 * 0.71,
                dc_offset: 0.13,
                ..Channel::default()
            },
        );
    }
}

#[test]
fn maximum_frame_clock_mismatch_and_long_constant_runs() {
    for (i, ppm) in [-3000.0, -1000.0, 1000.0, 3000.0].into_iter().enumerate() {
        let payload = if i % 2 == 0 {
            vec![0x00; 256]
        } else {
            vec![0xff; 256]
        };
        assert_channel(
            &Packet::new(0xaaaa_aaaa, 0, payload),
            Channel {
                leading_samples: 337,
                trailing_samples: 501,
                clock_ppm: ppm,
                phase_radians: 2.1,
                snr_db: Some(15.0),
                ..Channel::default()
            },
        );
    }
}

#[test]
fn combined_channel_bias_echo_noise_and_gain_ramp() {
    for (seed, bias) in [(1, 0.5), (2, 2.0), (3, 0.25), (4, 4.0)] {
        assert_channel(
            &packet(31),
            Channel {
                seed,
                leading_samples: 6037,
                trailing_samples: 2111,
                gain: 0.3,
                end_gain_ratio: 0.3,
                one_carrier_gain: bias,
                phase_radians: 1.3,
                clock_ppm: 1500.0,
                echo_delay_samples: 137,
                echo_gain: 0.3,
                snr_db: Some(15.0),
                dc_offset: 0.08,
                ..Channel::default()
            },
        );
    }
}

#[test]
fn tracks_relative_carrier_startup_with_echo_and_noise() {
    for (seed, initial_gain) in [(1, 0.15), (17, 0.25), (42, 0.5)] {
        assert_channel(
            &packet(17),
            Channel {
                seed,
                leading_samples: 6037,
                trailing_samples: 2111,
                gain: 0.1,
                one_carrier_start_gain: initial_gain,
                one_carrier_gain: 3.0,
                carrier_ramp_symbols: 64,
                echo_delay_samples: 384,
                echo_gain: 0.3,
                phase_radians: 1.3,
                snr_db: Some(15.0),
                ..Channel::default()
            },
        );
    }
}

#[test]
fn startup_training_does_not_replace_corrupted_preamble_bits() {
    for index in [0, 16, 63] {
        let mut bits = bytes_to_bits(&encode_frame(&packet(17)).unwrap());
        bits[index] = !bits[index];
        let samples = modulate(&bits);
        assert!(
            matches!(
                receive(&samples),
                Err(ReceiveError::Frame(FrameError::InvalidPreamble))
            ),
            "corrupted preamble bit {index} must not be reconstructed from training labels"
        );
    }
}

#[test]
fn acquisition_searches_past_early_noise_burst() {
    let packet = packet(17);
    let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
    let mut signal = Channel {
        leading_samples: 72013,
        snr_db: Some(0.0),
        ..Channel::default()
    }
    .transmit(&bits)
    .unwrap();
    let mut noise = Noise::new(77);
    for x in &mut signal[5000..15000] {
        *x += (0.6 * noise.gaussian()) as f32;
    }
    assert_eq!(receive(&signal).unwrap(), packet);
}

#[test]
fn crc_corruption_is_never_returned_as_a_packet_and_search_continues() {
    let packet = packet(17);
    let mut bytes = encode_frame(&packet).unwrap();
    bytes[PREFIX_SIZE + 3] ^= 0x40;
    let mut signal = modulate(&bytes_to_bits(&bytes));
    assert!(matches!(
        receive(&signal),
        Err(ReceiveError::Frame(FrameError::CrcMismatch { .. }))
    ));
    signal.extend(vec![0.0; 733]);
    signal.extend(modulate(&bytes_to_bits(&encode_frame(&packet).unwrap())));
    let attempts = inspect(&signal, ReceiverOptions::default()).unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(attempts[0].result.is_err());
    assert_eq!(attempts[1].result.as_ref().unwrap(), &packet);
    assert_eq!(receive(&signal).unwrap(), packet);
}

#[test]
fn missing_tail_and_invalid_header_fail_closed() {
    let mut bytes = encode_frame(&packet(17)).unwrap();
    let mut signal = modulate(&bytes_to_bits(&bytes));
    signal.truncate(signal.len() - 480);
    assert!(matches!(
        receive(&signal),
        Err(ReceiveError::Truncated { .. })
    ));
    bytes[PREFIX_SIZE - 2] = 0xff;
    bytes[PREFIX_SIZE - 1] = 0xff;
    assert!(matches!(
        receive(&modulate(&bytes_to_bits(&bytes))),
        Err(ReceiveError::Frame(FrameError::InvalidPayloadLength))
    ));
}

#[test]
fn rejects_silence_dc_noise_tones_and_nonfinite_input() {
    for signal in [
        vec![],
        vec![0.0; 96000],
        vec![0.5; 96000],
        modulate(&[false; 200]),
        modulate(&[true; 200]),
    ] {
        assert!(matches!(
            receive(&signal),
            Err(ReceiveError::AcquisitionFailed)
        ));
    }
    for seed in 1..=5 {
        let mut rng = Noise::new(seed);
        let signal: Vec<_> = (0..96000).map(|_| (rng.gaussian() * 0.2) as f32).collect();
        assert!(matches!(
            receive(&signal),
            Err(ReceiveError::AcquisitionFailed)
        ));
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(receive(&[value]), Err(ReceiveError::InvalidSamples));
    }
}

#[test]
fn alternating_preamble_without_sync_is_not_a_frame() {
    assert!(matches!(
        receive(&modulate(&[true, false].repeat(150))),
        Err(ReceiveError::AcquisitionFailed)
    ));
}

#[test]
fn bit_comparison_accounts_for_unobserved_bits() {
    let stats = compare_bits(&[true, false, true], &[false, false]);
    assert_eq!(stats.compared_bits, 2);
    assert_eq!(stats.bit_errors, 1);
    assert_eq!(stats.missing_bits, 1);
    assert_eq!(stats.ber(), Some(0.5));
    assert_eq!(compare_bits(&[true], &[]).ber(), None);
}

#[test]
#[ignore = "optional exhaustive release-mode sweep: 257 payload lengths and all 480 sample offsets"]
fn exhaustive_lengths_and_alignments() {
    for length in 0..=MAX_PAYLOAD_SIZE {
        let packet = packet(length);
        let signal = modulate(&bytes_to_bits(&encode_frame(&packet).unwrap()));
        assert_eq!(receive(&signal).unwrap(), packet, "length={length}");
    }
    for offset in 0..480 {
        assert_channel(
            &packet(1),
            Channel {
                leading_samples: offset,
                phase_radians: offset as f64 * 0.19,
                ..Channel::default()
            },
        );
    }
}

#[test]
#[ignore = "optional extended noise-only false-acquisition experiment"]
fn extended_false_acquisition() {
    for seed in 1..=200 {
        let mut rng = Noise::new(seed);
        let signal: Vec<_> = (0..240000).map(|_| (rng.gaussian() * 0.2) as f32).collect();
        assert!(
            matches!(receive(&signal), Err(ReceiveError::AcquisitionFailed)),
            "seed={seed}"
        );
    }
}

use tonequill_core::{
    protocol::{bitstream::bytes_to_bits, framing::encode_frame, packet::Packet},
    receiver::{ReceiverOptions, inspect},
    simulation::{Channel, Echo},
};

#[test]
fn preamble_ratio_acquires_biased_carrier_memory_before_energy_calibration() {
    for (seed, clock_ppm) in [(1, -80.0), (17, 120.0)] {
        for length in [17, 128] {
            for bias in [3.0, 4.0] {
                for echo_gain in [0.4, 0.5] {
                    let mut rng = tonequill_core::simulation::Noise::new(seed);
                    let packet = Packet::new(
                        0x916d7528,
                        3,
                        (0..length).map(|_| (rng.uniform() * 256.0) as u8).collect(),
                    );
                    let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
                    let samples = Channel {
                        seed,
                        leading_samples: 6037,
                        trailing_samples: 9600,
                        gain: 0.03,
                        one_carrier_gain: bias,
                        echo_delay_samples: 480,
                        echo_gain,
                        clock_ppm,
                        phase_radians: 1.3,
                        snr_db: Some(20.0),
                        ..Channel::default()
                    }
                    .transmit(&bits)
                    .unwrap();
                    assert!(matches!(
                        inspect(
                            &samples,
                            ReceiverOptions {
                                equalize: false,
                                ..ReceiverOptions::default()
                            }
                        ),
                        Err(tonequill_core::receiver::ReceiveError::AcquisitionFailed)
                    ));
                    let attempts =
                        inspect(&samples, ReceiverOptions::default()).unwrap_or_else(|e| {
                            panic!(
                                "seed={seed}, length={length}, bias={bias}, echo={echo_gain}: {e}"
                            )
                        });
                    let frame = attempts.iter().find(|a| a.result.as_ref() == Ok(&packet)).unwrap_or_else(|| panic!("seed={seed}, length={length}, bias={bias}, echo={echo_gain}: {attempts:?}"));
                    assert!(frame.acquisition.trained_acquisition_ratio.is_some());
                    assert_eq!(frame.bits(), bits);
                }
            }
        }
    }
}

#[test]
fn trained_acquisition_and_timing_never_replace_corrupted_training_bits() {
    let bits = bytes_to_bits(&encode_frame(&Packet::new(0x81725194, 7, vec![0x73; 17])).unwrap());
    let channel = Channel {
        leading_samples: 6037,
        trailing_samples: 9600,
        gain: 0.03,
        one_carrier_gain: 3.0,
        echo_delay_samples: 480,
        echo_gain: 0.5,
        phase_radians: 1.3,
        ..Channel::default()
    };
    for index in 0..80 {
        let mut corrupt = bits.clone();
        corrupt[index] = !corrupt[index];
        let samples = channel.transmit(&corrupt).unwrap();
        assert!(
            !inspect(&samples, ReceiverOptions::default())
                .is_ok_and(|a| a.iter().any(|a| a.result.is_ok())),
            "training search replaced transmitted bit {index}"
        );
    }
}

#[test]
fn diagnostic_probe_is_bounded_and_accepts_non_byte_aligned_counts() {
    use tonequill_core::receiver::probe_energy_symbols;
    let bits = bytes_to_bits(&encode_frame(&Packet::new(7, 0, vec![0x35; 59])).unwrap());
    let samples = tonequill_core::modulation::bfsk::modulate(&bits);
    let acq = inspect(&samples, ReceiverOptions::default())
        .unwrap()
        .remove(0)
        .acquisition;
    for count in [0, 1, 3, 80, usize::MAX] {
        let (symbols, _) =
            probe_energy_symbols(&samples, acq.clone(), ReceiverOptions::default(), count).unwrap();
        assert!(symbols.len() <= count.min(tonequill_core::protocol::framing::MAX_FRAME_SIZE * 8));
        assert_eq!(
            symbols.iter().map(|s| s.bit).collect::<Vec<_>>(),
            bits[..symbols.len()]
        );
    }
}

#[test]
fn channel_training_never_replaces_any_strongly_transmitted_preamble_or_sync_bit() {
    let bits = bytes_to_bits(&encode_frame(&Packet::new(0x7598a347, 17, vec![0x72; 59])).unwrap());
    for index in 0..80 {
        let mut corrupt = bits.clone();
        corrupt[index] = !corrupt[index];
        let samples = tonequill_core::modulation::bfsk::modulate(&corrupt);
        assert!(
            !inspect(&samples, ReceiverOptions::default())
                .is_ok_and(|a| a.iter().any(|a| a.result.is_ok())),
            "known training label replaced received bit {index}"
        );
    }
}

#[test]
fn independently_framed_packets_reacquire_with_200_ms_guard_and_corruption_on_either_side() {
    use tonequill_core::{receiver::scan, transfer::Fragmenter};
    let packets: Vec<_> = Fragmenter::new(0x81730129, "hello.txt", b"arbitrary bytes!!")
        .unwrap()
        .collect();
    assert_eq!(packets.len(), 2);
    for corrupt in [None, Some(0), Some(1)] {
        let mut recording = Vec::new();
        for (i, packet) in packets.iter().enumerate() {
            let mut bits = bytes_to_bits(&encode_frame(packet).unwrap());
            if corrupt == Some(i) {
                bits[167] = !bits[167];
            }
            let channel = Channel {
                seed: 101 + i as u64,
                gain: 0.1,
                leading_samples: if i == 0 { 9600 } else { 9600 - 1920 },
                trailing_samples: if i == 1 { 9600 } else { 0 },
                one_carrier_start_gain: 2.0,
                one_carrier_gain: 0.6,
                carrier_ramp_symbols: 400,
                phase_radians: 0.7 + i as f64,
                snr_db: Some(20.0),
                echoes: vec![
                    Echo {
                        delay_samples: 480,
                        gain: 0.35,
                        end_gain_ratio: 0.8,
                    },
                    Echo {
                        delay_samples: 960,
                        gain: 0.3,
                        end_gain_ratio: 1.1,
                    },
                    Echo {
                        delay_samples: 1920,
                        gain: 0.12,
                        end_gain_ratio: 0.7,
                    },
                ],
                ..Channel::default()
            };
            recording.extend(channel.transmit(&bits).unwrap());
        }
        let attempts = scan(&recording, ReceiverOptions::default()).unwrap();
        assert_eq!(attempts.len(), 2, "{corrupt:?}: {attempts:?}");
        for (i, attempt) in attempts.iter().enumerate() {
            if corrupt == Some(i) {
                assert!(
                    attempt.result.is_err(),
                    "a transmitted CRC error must remain an error"
                );
            } else {
                assert_eq!(attempt.result.as_ref().unwrap(), &packets[i]);
                assert_eq!(
                    attempt.bits(),
                    bytes_to_bits(&encode_frame(&packets[i]).unwrap())
                );
            }
        }
        let mut live = tonequill_core::receiver::LiveDecoder::default();
        let mut incremental = Vec::new();
        for chunk in recording.chunks(12000) {
            incremental.extend(live.push(chunk).unwrap());
        }
        incremental.extend(live.flush().unwrap());
        let valid: Vec<_> = incremental
            .iter()
            .filter_map(|a| a.result.as_ref().ok())
            .collect();
        let expected: Vec<_> = packets
            .iter()
            .enumerate()
            .filter_map(|(i, p)| (corrupt != Some(i)).then_some(p))
            .collect();
        assert_eq!(
            valid, expected,
            "incremental synthetic replay, corrupt={corrupt:?}"
        );
    }
}

#[test]
fn time_varying_multipath_fails_energy_receiver_and_recovers_without_payload_training() {
    for (seed, ppm) in [(1, -80.0), (17, 33.0), (42, 120.0)] {
        for length in [17, 59, 128, 256] {
            for gain in [0.35, 0.5] {
                for final_gain in [0.6, 1.0] {
                    let packet = Packet::new(0x73ac8192, 7, {
                        let mut rng = tonequill_core::simulation::Noise::new(seed);
                        (0..length).map(|_| (rng.uniform() * 256.0) as u8).collect()
                    });
                    let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
                    let channel = Channel {
                        seed,
                        leading_samples: 6037,
                        trailing_samples: 9600,
                        gain: 0.1,
                        one_carrier_start_gain: 2.0,
                        one_carrier_gain: final_gain,
                        carrier_ramp_symbols: 400,
                        clock_ppm: ppm,
                        phase_radians: 1.3,
                        snr_db: Some(20.0),
                        echoes: vec![
                            Echo {
                                delay_samples: 480,
                                gain,
                                end_gain_ratio: 0.8,
                            },
                            Echo {
                                delay_samples: 960,
                                gain: 0.3,
                                end_gain_ratio: 1.1,
                            },
                            Echo {
                                delay_samples: 1920,
                                gain: 0.12,
                                end_gain_ratio: 0.7,
                            },
                        ],
                        ..Channel::default()
                    };
                    let wav = channel.transmit(&bits).unwrap();
                    let old = inspect(
                        &wav,
                        ReceiverOptions {
                            equalize: false,
                            ..ReceiverOptions::default()
                        },
                    );
                    let new = inspect(&wav, ReceiverOptions::default());
                    let good = |a: &Result<Vec<tonequill_core::receiver::FrameAttempt>, _>| {
                        a.as_ref()
                            .is_ok_and(|a| a.iter().any(|a| a.result.as_ref() == Ok(&packet)))
                    };
                    assert!(
                        !good(&old),
                        "old behavior must reproduce the failure: {channel:?}"
                    );
                    assert!(good(&new), "new receiver failed: {channel:?}; {new:?}");
                    let recovered = new
                        .as_ref()
                        .unwrap()
                        .iter()
                        .find(|a| a.result.is_ok())
                        .unwrap();
                    assert_eq!(
                        recovered.bits(),
                        bits,
                        "every framing and payload bit must match"
                    );
                    println!(
                        "seed={seed} length={length} echo={gain} final={final_gain} old={} new={} eq={:?}",
                        good(&old),
                        good(&new),
                        new.as_ref()
                            .ok()
                            .and_then(|a| a[0].equalizer.as_ref())
                            .map(|m| (m.memory_symbols, m.validation_error, m.samples_per_symbol))
                    );
                }
            }
        }
    }
}

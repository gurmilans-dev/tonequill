use tonequill_core::{
    protocol::{bitstream::bytes_to_bits, framing::encode_frame, packet::Packet},
    receiver::{ReceiverOptions, inspect},
    simulation::{Channel, Noise},
};

fn channel(seed: u64, delay: usize) -> Channel {
    Channel {
        seed,
        gain: 0.05,
        leading_samples: 6037,
        trailing_samples: 24000,
        echo_delay_samples: delay,
        echo_gain: 1.0,
        one_carrier_gain: 2.0,
        snr_db: Some(20.0),
        phase_radians: 1.3,
        ..Channel::default()
    }
}

#[test]
fn central_half_acquires_fractional_symbol_echo_at_unchanged_gates() {
    for seed in [17, 73] {
        for length in [17, 59] {
            for delay in [300, 360] {
                let mut rng = Noise::new(seed);
                let packet = Packet::new(
                    0x51398276,
                    3,
                    (0..length).map(|_| (rng.uniform() * 256.0) as u8).collect(),
                );
                let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
                let samples = channel(seed, delay).transmit(&bits).unwrap();
                let attempts = inspect(&samples, ReceiverOptions::default()).unwrap();
                let received = attempts
                    .iter()
                    .find(|a| a.result.as_ref() == Ok(&packet))
                    .unwrap_or_else(|| {
                        panic!("seed={seed} length={length} delay={delay}: {attempts:?}")
                    });
                assert_eq!(received.acquisition.acquisition_window_samples, 240);
                assert!(received.acquisition.preamble_matches >= 56);
                assert!(received.acquisition.sync_matches >= 13);
                assert!(received.acquisition.quality >= 0.5);
                assert_eq!(received.bits(), bits);
                let model = received.equalizer.as_ref().unwrap();
                assert_eq!(model.training_errors, 0);
                assert!(model.validation_error < 1.0);
            }
        }
    }
}

#[test]
fn central_half_search_does_not_repair_corrupted_training_or_crc() {
    let packet = Packet::new(0x68371924, 9, b"Unrelated regression payload".to_vec());
    let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
    let channel = Channel {
        snr_db: None,
        ..channel(101, 300)
    };
    for index in (0..80).chain([180, bits.len() - 1]) {
        let mut corrupt = bits.clone();
        corrupt[index] = !corrupt[index];
        let attempts = inspect(
            &channel.transmit(&corrupt).unwrap(),
            ReceiverOptions::default(),
        );
        if let Ok(attempts) = &attempts {
            for a in attempts.iter().filter(|a| a.result.is_ok()) {
                eprintln!(
                    "corrupt={index}: acq={:?}; eq={:?}; first={:?}",
                    a.acquisition,
                    a.equalizer,
                    &a.symbols[..3]
                );
            }
        }
        assert!(
            !attempts.is_ok_and(|a| a.iter().any(|a| a.result.is_ok())),
            "accepted corrupted bit {index}"
        );
    }
}

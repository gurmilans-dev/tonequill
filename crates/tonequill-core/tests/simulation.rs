use tonequill_core::{
    evaluation::{EvaluationConfig, evaluate},
    receiver::ReceiverOptions,
    simulation::Channel,
};

#[test]
fn deterministic_noise_obeys_active_signal_snr_with_padding() {
    let channel = Channel {
        seed: 97,
        leading_samples: 48000,
        trailing_samples: 6000,
        gain: 0.2,
        one_carrier_gain: 2.0,
        echo_delay_samples: 137,
        echo_gain: 0.3,
        clock_ppm: 2000.0,
        phase_radians: 2.3,
        ..Channel::default()
    };
    let bits = [true, false].repeat(100);
    let clean = channel.transmit(&bits).unwrap();
    let noisy_channel = Channel {
        snr_db: Some(15.0),
        ..channel.clone()
    };
    let noisy = noisy_channel.transmit(&bits).unwrap();
    assert_eq!(noisy, noisy_channel.transmit(&bits).unwrap());
    let active = &clean[channel.leading_samples..clean.len() - channel.trailing_samples];
    let signal_power =
        active.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / active.len() as f64;
    let noise_power = noisy[..channel.leading_samples]
        .iter()
        .map(|&x| (x as f64).powi(2))
        .sum::<f64>()
        / channel.leading_samples as f64;
    let measured_snr = 10.0 * (signal_power / noise_power).log10();
    assert!(
        (measured_snr - 15.0).abs() < 0.2,
        "measured {measured_snr} dB"
    );
}

#[test]
fn evaluation_accounts_for_every_packet_and_missing_bit() {
    for snr in [15.0, -20.0] {
        let stats = evaluate(&EvaluationConfig {
            trials: 3,
            seed: 1,
            payload_bytes: 17,
            channel: Channel {
                snr_db: Some(snr),
                leading_samples: 3457,
                trailing_samples: 1200,
                ..Channel::default()
            },
            receiver: ReceiverOptions::default(),
        })
        .unwrap();
        assert_eq!(stats.total_packets, 3);
        assert_eq!(stats.total_bits, 3 * 328);
        assert_eq!(stats.compared_bits + stats.missing_bits, stats.total_bits);
        assert_eq!(
            stats.valid_packets
                + stats.acquisition_failures
                + stats.crc_failures
                + stats.protocol_failures
                + stats.truncated_frames
                + stats.incorrect_valid_packets,
            stats.total_packets
        );
        assert_eq!(stats.incorrect_valid_packets, 0);
        if snr == 15.0 {
            assert_eq!(stats.valid_packets, 3);
            assert_eq!(stats.ber(), Some(0.0));
        }
    }
}

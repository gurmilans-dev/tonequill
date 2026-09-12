use tonequill_core::{
    protocol::{bitstream::bytes_to_bits, framing::encode_frame, packet::Packet},
    receiver::{ReceiverOptions, inspect},
    simulation::{Channel, Echo, Noise},
};

#[test]
fn ambiguous_training_preserves_other_complete_crc_valid_hypotheses() {
    let mut rng = Noise::new(42);
    let packet = Packet::new(
        0x9571ba32,
        3,
        (0..59).map(|_| (rng.uniform() * 256.0) as u8).collect(),
    );
    let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
    // Fractional-symbol reflections and a carrier gain ramp can make the
    // best preamble-response fit a poor predictor of later bit patterns.
    let channel = Channel {
        seed: 42,
        leading_samples: 6037,
        trailing_samples: 9600,
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
    };
    let samples = channel.transmit(&bits).unwrap();
    let attempts = inspect(&samples, ReceiverOptions::default()).unwrap();
    let recovered = attempts
        .iter()
        .find(|a| a.result.as_ref() == Ok(&packet))
        .unwrap();
    assert_eq!(recovered.bits(), bits);
    let metrics = recovered.equalizer.as_ref().unwrap();
    // The previous single-best policy would discard this valid hypothesis.
    assert!(metrics.hypothesis_rank > 1);
    assert!(metrics.hypothesis_rank <= metrics.hypothesis_count);
    assert!(metrics.hypothesis_count <= 21 * 2 * 9);
    assert_eq!(metrics.training_errors, 0);
    assert!(metrics.validation_error < 1.0);

    // The list must not repair an intentionally transmitted CRC error.
    let mut corrupt = bits;
    corrupt[180] = !corrupt[180];
    let attempts = inspect(
        &channel.transmit(&corrupt).unwrap(),
        ReceiverOptions::default(),
    )
    .unwrap();
    assert!(attempts.iter().all(|a| a.result.is_err()));
}

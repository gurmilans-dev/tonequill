use tonequill_core::{
    protocol::{bitstream::bytes_to_bits, framing::encode_frame, packet::Packet},
    receiver::{ReceiverOptions, inspect, probe_energy_symbols},
    simulation::Channel,
};

#[test]
fn energy_probe_matches_received_symbols_on_both_observation_grids() {
    let packet = Packet::new(0x7214c98a, 4, (0..59).map(|i| (i * 73) as u8).collect());
    let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
    let samples = Channel {
        seed: 813,
        leading_samples: 6137,
        trailing_samples: 9600,
        gain: 0.05,
        phase_radians: 1.3,
        clock_ppm: 1250.0,
        snr_db: Some(24.0),
        ..Channel::default()
    }
    .transmit(&bits)
    .unwrap();
    for equalize in [true, false] {
        let options = ReceiverOptions {
            equalize,
            ..ReceiverOptions::default()
        };
        let attempts = inspect(&samples, options).unwrap();
        let received = attempts
            .iter()
            .find(|a| a.result.as_ref() == Ok(&packet))
            .unwrap();
        assert!(
            received.equalizer.is_none(),
            "test requires ordinary energy decisions"
        );
        let (probed, period) =
            probe_energy_symbols(&samples, received.acquisition.clone(), options, bits.len())
                .unwrap();
        assert_eq!(probed.len(), received.symbols.len());
        for (actual, diagnostic) in received.symbols.iter().zip(&probed) {
            assert_eq!(actual.bit, diagnostic.bit);
            assert_eq!(
                actual.center_sample, diagnostic.center_sample,
                "equalize={equalize}, index={}",
                actual.index
            );
            assert_eq!(
                actual.carrier_power, diagnostic.carrier_power,
                "equalize={equalize}, index={}",
                actual.index
            );
            assert_eq!(actual.soft_value, diagnostic.soft_value);
            assert_eq!(actual.timing_correction, diagnostic.timing_correction);
        }
        assert_eq!(period, received.final_samples_per_symbol);
    }
}

//! Frozen pre-engineering-pass acquisition, for reproducible A/B measurements.
#[path = "support/baseline.rs"]
mod baseline;
use tonequill_core::{
    demodulation::bfsk::demodulate_from_offset,
    protocol::{
        bitstream::{bits_to_bytes, bytes_to_bits},
        framing::{decode_frame, encode_frame},
        packet::Packet,
    },
    simulation::Channel,
};

fn main() {
    let modern = std::env::args().any(|a| a == "--modern");
    println!("case,packets,valid,acquisition_failures,compared_bits,bit_errors,ber,per");
    for (name, channel, length) in [
        ("ideal", Channel::default(), 17),
        (
            "alignment",
            Channel {
                leading_samples: 3457,
                trailing_samples: 1703,
                ..Channel::default()
            },
            17,
        ),
        (
            "low_gain",
            Channel {
                leading_samples: 3457,
                gain: 0.005,
                ..Channel::default()
            },
            17,
        ),
        (
            "noise_15db",
            Channel {
                leading_samples: 6037,
                trailing_samples: 177,
                snr_db: Some(15.0),
                ..Channel::default()
            },
            17,
        ),
        (
            "noise_0db",
            Channel {
                leading_samples: 6037,
                snr_db: Some(0.0),
                ..Channel::default()
            },
            17,
        ),
        (
            "clock_2000ppm",
            Channel {
                leading_samples: 3457,
                trailing_samples: 2000,
                clock_ppm: 2000.0,
                ..Channel::default()
            },
            256,
        ),
        (
            "clock_minus_2000ppm",
            Channel {
                leading_samples: 3457,
                trailing_samples: 4000,
                clock_ppm: -2000.0,
                ..Channel::default()
            },
            256,
        ),
        (
            "combined",
            Channel {
                leading_samples: 6037,
                trailing_samples: 2111,
                gain: 0.3,
                one_carrier_gain: 2.0,
                phase_radians: 1.3,
                snr_db: Some(15.0),
                clock_ppm: 1500.0,
                echo_delay_samples: 137,
                echo_gain: 0.3,
                ..Channel::default()
            },
            17,
        ),
    ] {
        let (mut valid, mut missed, mut compared, mut errors) = (0, 0, 0, 0);
        for seed in 1..=10 {
            let payload = (0..length)
                .map(|i| ((i * 73 + seed * 19) & 255) as u8)
                .collect();
            let packet = Packet::new(seed as u32, 0, payload);
            let frame = encode_frame(&packet).unwrap();
            let tx = bytes_to_bits(&frame);
            let rx = Channel {
                seed: seed as u64,
                ..channel.clone()
            }
            .transmit(&tx)
            .unwrap();
            if modern {
                use tonequill_core::receiver::{ReceiverOptions, compare_bits, inspect};
                match inspect(&rx, ReceiverOptions::default()) {
                    Ok(attempts) => {
                        let attempt = attempts
                            .iter()
                            .find(|a| a.result.is_ok())
                            .unwrap_or(&attempts[0]);
                        let comparison = compare_bits(&tx, &attempt.bits());
                        compared += comparison.compared_bits;
                        errors += comparison.bit_errors;
                        valid += usize::from(attempt.result.as_ref().is_ok_and(|p| *p == packet));
                        if attempt.result.is_err() {
                            eprintln!(
                                "{name}/{seed}: {:?}, {:?}",
                                attempt.result, attempt.acquisition
                            );
                        }
                    }
                    Err(_) => missed += 1,
                }
                continue;
            }
            if let Some(sync) = baseline::find_frame_start(&rx) {
                let _metrics = (sync.matched_bits, sync.tested_bits, sync.quality);
                let bits = demodulate_from_offset(&rx, sync.start_sample);
                let n = bits.len().min(tx.len());
                compared += n;
                errors += tx[..n]
                    .iter()
                    .zip(&bits[..n])
                    .filter(|(a, b)| a != b)
                    .count();
                if bits.len() >= tx.len() {
                    let bytes = bits_to_bytes(&bits[..tx.len()]).unwrap();
                    valid += usize::from(decode_frame(&bytes).is_ok_and(|p| p == packet));
                }
            } else {
                missed += 1;
            }
        }
        println!(
            "{name},10,{valid},{missed},{compared},{errors},{:.8},{:.4}",
            errors as f64 / compared.max(1) as f64,
            1.0 - valid as f64 / 10.0
        );
    }
}

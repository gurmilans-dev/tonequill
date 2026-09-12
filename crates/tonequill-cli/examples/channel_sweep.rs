//! Deterministic receiver comparison across varied payloads and channel conditions.
//! Usage: channel_sweep [case-count]; emits CSV and never uses reference payloads
//! to select receiver hypotheses. Success requires the exact generated packet.
use tonequill_core::{
    protocol::{bitstream::bytes_to_bits, framing::encode_frame, packet::Packet},
    receiver::{ReceiverOptions, inspect},
    simulation::{Channel, Echo, Noise},
};
fn main() {
    let count: usize = std::env::args().nth(1).map_or(128, |s| s.parse().unwrap());
    println!("case,seed,length,delay,echo,carrier,snr,clock,leading,ok,window,start");
    for case in 0..count {
        let seed = 9001 + case as u64;
        let mut rng = Noise::new(seed);
        let mut pick = |n: usize| (rng.uniform() * n as f64) as usize;
        let length = [17, 59, 127, 256][pick(4)];
        let delay = [180, 240, 300, 360, 420, 480, 667, 960][pick(8)];
        let echo = [0.3, 0.6, 0.8, 1.0, 1.2][pick(5)];
        let carrier = [0.15, 0.4, 0.8, 1.0, 2.0, 4.0][pick(6)];
        let snr = [12.0, 16.0, 20.0, 28.0][pick(4)];
        let clock = [-2000.0, -50.0, 0.0, 1000.0][pick(4)];
        let leading = 6000 + pick(480);
        let phase = rng.uniform() * std::f64::consts::TAU;
        let packet = Packet::new(
            0x8127ef03,
            7,
            (0..length).map(|_| (rng.uniform() * 256.0) as u8).collect(),
        );
        let bits = bytes_to_bits(&encode_frame(&packet).unwrap());
        let samples = Channel {
            seed,
            gain: 0.05,
            leading_samples: leading,
            trailing_samples: 24000,
            phase_radians: phase,
            one_carrier_gain: carrier,
            snr_db: Some(snr),
            clock_ppm: clock,
            echoes: vec![Echo {
                delay_samples: delay,
                gain: echo,
                end_gain_ratio: 1.0,
            }],
            ..Channel::default()
        }
        .transmit(&bits)
        .unwrap();
        let attempts = inspect(&samples, ReceiverOptions::default()).unwrap_or_default();
        let received = attempts.iter().find(|a| a.result.as_ref() == Ok(&packet));
        println!(
            "{case},{seed},{length},{delay},{echo},{carrier},{snr},{clock},{leading},{},{},{}",
            received.is_some(),
            received.map_or(0, |a| a.acquisition.acquisition_window_samples),
            received.map_or(0.0, |a| a.acquisition.start_sample)
        );
    }
}

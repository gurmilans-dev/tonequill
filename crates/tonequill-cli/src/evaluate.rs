use anyhow::{Result, bail};
use clap::Args;
use tonequill_core::{
    evaluation::{EvaluationConfig, evaluate},
    receiver::ReceiverOptions,
    simulation::Channel,
};

#[derive(Debug, Args)]
pub struct EvaluateArgs {
    #[arg(long, default_value_t = 100)]
    trials: usize,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    #[arg(long, default_value_t = 17)]
    payload_bytes: usize,
    /// Comma-separated broadband SNR values in dB, measured during active audio.
    #[arg(
        long,
        value_delimiter = ',',
        allow_hyphen_values = true,
        default_value = "15,5,0,-5,-10"
    )]
    snr_db: Vec<f64>,
    #[arg(long, default_value_t = 6037)]
    leading_samples: usize,
    #[arg(long, default_value_t = 2111)]
    trailing_samples: usize,
    #[arg(long, default_value_t = 1.0)]
    gain: f64,
    #[arg(long, default_value_t = 1.0)]
    end_gain_ratio: f64,
    #[arg(long, default_value_t = 1.0)]
    one_carrier_gain: f64,
    #[arg(long, default_value_t = 1.0)]
    one_carrier_start_gain: f64,
    /// Duration of a relative-carrier startup ramp; zero disables it.
    #[arg(long, default_value_t = 0)]
    carrier_ramp_symbols: usize,
    #[arg(long, default_value_t = 1.3)]
    phase: f64,
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    clock_ppm: f64,
    #[arg(long, default_value_t = 0)]
    echo_delay_samples: usize,
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    echo_gain: f64,
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    dc_offset: f64,
    #[arg(long)]
    no_calibration: bool,
    #[arg(long)]
    no_clock: bool,
    #[arg(long)]
    no_tracking: bool,
    #[arg(long)]
    full_symbols: bool,
    /// A/B control: disable labelled-preamble decision threshold fitting.
    #[arg(long)]
    power_ratio_threshold: bool,
    #[arg(long)]
    no_equalizer: bool,
}

pub fn run(args: EvaluateArgs) -> Result<()> {
    eprintln!(
        "{} trials/SNR, seed={}, payload={} bytes. BER is conditional on compared bits; missing bits are separate.",
        args.trials, args.seed, args.payload_bytes
    );
    println!(
        "snr_db,total_packets,valid_packets,total_bits,compared_bits,bit_errors,missing_bits,ber,per,acquisition_failures,crc_failures,protocol_failures,truncated_frames,incorrect_valid_packets"
    );
    for snr in args.snr_db {
        let stats = evaluate(&EvaluationConfig {
            trials: args.trials,
            seed: args.seed,
            payload_bytes: args.payload_bytes,
            channel: Channel {
                leading_samples: args.leading_samples,
                trailing_samples: args.trailing_samples,
                gain: args.gain,
                end_gain_ratio: args.end_gain_ratio,
                one_carrier_gain: args.one_carrier_gain,
                one_carrier_start_gain: args.one_carrier_start_gain,
                carrier_ramp_symbols: args.carrier_ramp_symbols,
                phase_radians: args.phase,
                clock_ppm: args.clock_ppm,
                echo_delay_samples: args.echo_delay_samples,
                echo_gain: args.echo_gain,
                snr_db: Some(snr),
                dc_offset: args.dc_offset,
                ..Channel::default()
            },
            receiver: ReceiverOptions {
                calibrate_carriers: !args.no_calibration,
                estimate_clock: !args.no_clock,
                track_timing: !args.no_tracking,
                full_symbol_decisions: args.full_symbols,
                train_decision_threshold: !args.power_ratio_threshold,
                equalize: !args.no_equalizer,
            },
        })?;
        println!(
            "{snr},{},{},{},{},{},{},{},{:.6},{},{},{},{},{}",
            stats.total_packets,
            stats.valid_packets,
            stats.total_bits,
            stats.compared_bits,
            stats.bit_errors,
            stats.missing_bits,
            stats
                .ber()
                .map_or(String::from("NA"), |v| format!("{v:.8}")),
            stats.per().unwrap(),
            stats.acquisition_failures,
            stats.crc_failures,
            stats.protocol_failures,
            stats.truncated_frames,
            stats.incorrect_valid_packets
        );
        if stats.incorrect_valid_packets != 0 {
            bail!("integrity failure: CRC-valid packet differs from TX");
        }
    }
    Ok(())
}

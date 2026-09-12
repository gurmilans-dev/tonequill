use anyhow::Result;
use clap::Args;
use std::io::{self, Write};
use tonequill_core::transfer::reliable::{
    Timing,
    evaluation::campaign,
    simulation::{LossModel, SimulationConfig, Strategy},
};

#[derive(Debug, Args)]
pub struct EvaluateTransferArgs {
    #[arg(long, value_delimiter = ',', default_value = "1024,10240")]
    file_sizes: Vec<usize>,
    #[arg(long, value_delimiter = ',', default_value = "1,4,8", value_parser = clap::value_parser!(u8).range(1..=32))]
    windows: Vec<u8>,
    /// Also measure the existing one-way scheme (receiver delivery, no sender confirmation).
    #[arg(long)]
    include_one_way: bool,
    #[arg(long, value_delimiter = ',', default_value = "0,0.05,0.1")]
    data_loss: Vec<f64>,
    /// Applied to status/Complete and forward control requests.
    #[arg(long, value_delimiter = ',', default_value = "0,0.05,0.1")]
    feedback_loss: Vec<f64>,
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u32).range(1..=100000))]
    trials: u32,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..=100))]
    retries: u32,
    #[arg(long, default_value_t = 40000)]
    timeout_ms: u64,
    #[arg(long, default_value_t = 300)]
    turnaround_ms: u64,
    #[arg(long, default_value_t = 10)]
    delay_ms: u64,
    #[arg(long, default_value_t = 0)]
    jitter_ms: u64,
    #[arg(long, default_value_t = 0.0)]
    corruption: f64,
    #[arg(long, default_value_t = 0.0)]
    duplication: f64,
}

pub fn run(args: EvaluateTransferArgs) -> Result<()> {
    let mut strategies: Vec<_> = args
        .windows
        .iter()
        .map(|&window| Strategy::SelectiveRepeat { window })
        .collect();
    if args.include_one_way {
        strategies.insert(0, Strategy::OneWay);
    }
    let mut writer = io::BufWriter::new(io::stdout().lock());
    writeln!(
        writer,
        "strategy,file_bytes,window,data_loss,feedback_loss,retry_limit,first_seed,trials,successful,failed,completion_rate,receiver_committed,sender_confirmed,data_frames,metadata_frames,control_frames,complete_frames,retransmitted_data,timeouts,retries,retry_exhausted,delivered_frames,unique_data_packets,dropped_frames,crc_rejects,half_duplex_rejects,useful_bytes,on_air_bits,modulation_seconds,guard_seconds,elapsed_seconds,mean_seconds,mean_retransmissions,mean_control_frames,goodput_bytes_per_second,attempted_throughput_bits_per_second,overhead_fraction,timeout_ms,turnaround_ms,delay_ms,jitter_ms,corruption,duplication"
    )?;
    for &size in &args.file_sizes {
        for &data_loss in &args.data_loss {
            for &control_loss in &args.feedback_loss {
                for &strategy in &strategies {
                    // Feedback loss is irrelevant to the one-way baseline: avoid duplicate rows.
                    if strategy == Strategy::OneWay && control_loss != args.feedback_loss[0] {
                        continue;
                    }
                    let config = SimulationConfig {
                        strategy,
                        retries: args.retries,
                        timing: Timing {
                            feedback_timeout_ms: args.timeout_ms,
                            turnaround_ms: args.turnaround_ms,
                            ..Default::default()
                        },
                        loss: LossModel {
                            data_loss,
                            control_loss,
                            corruption: args.corruption,
                            duplication: args.duplication,
                            delay_ms: args.delay_ms,
                            jitter_ms: args.jitter_ms,
                        },
                        ..Default::default()
                    };
                    let results = campaign(size, args.trials, args.seed, &config)?;
                    let (name, window) = match strategy {
                        Strategy::OneWay => ("one_way", 0),
                        Strategy::SelectiveRepeat { window: 1 } => ("stop_and_wait", 1),
                        Strategy::SelectiveRepeat { window } => ("selective_repeat", window),
                    };
                    writeln!(
                        writer,
                        "{name},{size},{window},{data_loss:.6},{control_loss:.6},{},{},{},{},{},{:.6},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{:.3},{:.3},{:.3},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{},{},{},{},{:.6},{:.6}",
                        args.retries,
                        args.seed,
                        results.trials,
                        results.successful,
                        results.trials - results.successful,
                        results.completion_rate(),
                        results.receiver_committed,
                        results.sender_confirmed,
                        results.data_frames,
                        results.metadata_frames,
                        results.control_frames,
                        results.complete_frames,
                        results.retransmitted_data,
                        results.timeouts,
                        results.retries,
                        results.retry_exhausted,
                        results.delivered_frames,
                        results.unique_data_packets,
                        results.dropped_frames,
                        results.crc_rejects,
                        results.half_duplex_rejects,
                        results.useful_bytes,
                        results.on_air_bits,
                        results.modulation_ms as f64 / 1000.0,
                        results.guard_ms as f64 / 1000.0,
                        results.elapsed_ms as f64 / 1000.0,
                        results.elapsed_ms as f64 / 1000.0 / f64::from(results.trials),
                        results.retransmitted_data as f64 / f64::from(results.trials),
                        results.control_frames as f64 / f64::from(results.trials),
                        results.goodput_bytes_per_second(),
                        results.throughput_bits_per_second(),
                        results.overhead_fraction(),
                        args.timeout_ms,
                        args.turnaround_ms,
                        args.delay_ms,
                        args.jitter_ms,
                        args.corruption,
                        args.duplication
                    )?;
                    writer.flush()?;
                    eprintln!(
                        "{name} window={window} file={size} loss={:.1}%/{:.1}%: {}/{} complete; {:.3} B/s useful goodput",
                        data_loss * 100.0,
                        control_loss * 100.0,
                        results.successful,
                        results.trials,
                        results.goodput_bytes_per_second()
                    );
                }
            }
        }
    }
    Ok(())
}

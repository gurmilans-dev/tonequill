mod audio;
mod diagnostics;
mod evaluate;
mod evaluate_transfer;
mod frame_diagnostics;
mod live;
mod transfers;
mod wav;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tonequill_core::receiver::ReceiverOptions;

#[derive(Debug, Parser)]
#[command(name = "tonequill", version, about = "Acoustic data link")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect every candidate against a clean multi-frame reference; retain failed-frame metrics.
    DiagnoseFrames(frame_diagnostics::DiagnoseFramesArgs),
    /// List microphone and speaker devices for live transfers.
    AudioDevices,
    /// Open both streams and measure microphone levels without saving audio.
    AudioCheck {
        #[command(flatten)]
        audio: audio::AudioArgs,
        #[arg(long, default_value_t = 3)]
        seconds: u64,
    },
    /// Send a file directly through speakers, with acoustic status and selective retry.
    Send(live::SendArgs),
    /// Listen on the microphone, reconstruct a file and return acoustic status.
    Receive(live::ReceiveArgs),
    /// Run seeded synthetic channel trials; write CSV statistics to stdout.
    Evaluate(evaluate::EvaluateArgs),
    /// Seeded whole-frame protocol loss campaigns; CSV stdout, summaries stderr.
    EvaluateTransfer(evaluate_transfer::EvaluateTransferArgs),
    /// Encode a file as an independently framed Tonequill transfer WAV.
    Encode {
        /// File to transmit.
        input: PathBuf,
        /// Destination WAV file.
        output: PathBuf,
        /// Emit one legacy raw-payload frame (max 256 bytes), for PHY diagnostics.
        #[arg(long)]
        legacy: bool,
        /// Explicit hexadecimal transfer ID; default is derived from file SHA-256.
        #[arg(long, value_parser = transfers::parse_transfer_id)]
        transfer_id: Option<u32>,
    },
    /// Recover a CRC- and SHA-256-verified file from a Tonequill WAV.
    Decode {
        /// Recorded or generated Tonequill WAV.
        input: PathBuf,
        /// Destination for the verified file.
        output: PathBuf,
        /// Select a hexadecimal transfer ID when a recording contains several.
        #[arg(long, value_parser = transfers::parse_transfer_id)]
        transfer_id: Option<u32>,
    },
    /// Compare a received legacy frame with a clean reference and report PHY metrics.
    Diagnose {
        /// Clean legacy reference WAV.
        reference: PathBuf,
        /// Received or channel-modified WAV.
        recording: PathBuf,
        /// Maximum number of low-confidence/error symbols to display.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Export all observed symbols as CSV (including on CRC failure).
        #[arg(long)]
        symbols_csv: Option<PathBuf>,
        /// Disable preamble-derived relative carrier calibration.
        #[arg(long)]
        no_calibration: bool,
        /// Disable preamble-based sample-clock estimation.
        #[arg(long)]
        no_clock: bool,
        /// Disable fractional symbol-timing tracking.
        #[arg(long)]
        no_tracking: bool,
        /// Integrate full symbols instead of their central half windows.
        #[arg(long)]
        full_symbols: bool,
        /// A/B control: use only the original desired-carrier power ratio.
        #[arg(long)]
        power_ratio_threshold: bool,
        /// Disable the trained channel-equalizer fallback.
        #[arg(long)]
        no_equalizer: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::DiagnoseFrames(args) => frame_diagnostics::run(args),
        Command::AudioDevices => audio::devices(),
        Command::AudioCheck { audio, seconds } => audio::check(&audio, seconds),
        Command::Send(args) => live::send(args),
        Command::Receive(args) => live::receive(args),
        Command::Evaluate(args) => evaluate::run(args),
        Command::EvaluateTransfer(args) => evaluate_transfer::run(args),
        Command::Encode {
            input,
            output,
            legacy,
            transfer_id,
        } => transfers::encode_file(&input, &output, legacy, transfer_id),
        Command::Decode {
            input,
            output,
            transfer_id,
        } => transfers::decode_file(&input, &output, transfer_id),
        Command::Diagnose {
            reference,
            recording,
            limit,
            symbols_csv,
            no_calibration,
            no_clock,
            no_tracking,
            full_symbols,
            power_ratio_threshold,
            no_equalizer,
        } => diagnostics::diagnose(
            &reference,
            &recording,
            limit,
            symbols_csv.as_deref(),
            ReceiverOptions {
                calibrate_carriers: !no_calibration,
                estimate_clock: !no_clock,
                track_timing: !no_tracking,
                full_symbol_decisions: full_symbols,
                train_decision_threshold: !power_ratio_threshold,
                equalize: !no_equalizer,
            },
        ),
    }
}

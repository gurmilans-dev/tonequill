use crate::audio::{self, AudioArgs};
use anyhow::{Context, Result, bail};
use clap::Args;
use link::{EventLog, Link};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tonequill_core::transfer::{
    MAX_FILE_SIZE,
    reliable::{ReceiveSession, SendSession, Timing},
};
use tonequill_live::{
    audio::{Audio, AudioOptions},
    runtime::{
        capture, commit_file, link, one_way, run_receiver, run_sender, validate_receive_paths,
    },
};

#[derive(Debug, Clone, Args)]
pub struct TimingArgs {
    /// Quiet TX/RX guard in milliseconds (use matching values on both endpoints).
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(0..=5000))]
    pub turnaround_ms: u64,
    /// Silence between frames in a window.
    #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u64).range(0..=2000))]
    pub frame_gap_ms: u64,
}
impl TimingArgs {
    fn timing(&self, timeout: u64) -> Timing {
        Timing {
            feedback_timeout_ms: timeout * 1000,
            turnaround_ms: self.turnaround_ms,
            inter_frame_guard_ms: self.frame_gap_ms,
            ..Default::default()
        }
    }
}
#[derive(Debug, Args)]
pub struct SendArgs {
    pub input: PathBuf,
    #[command(flatten)]
    pub audio: AudioArgs,
    #[command(flatten)]
    pub timing: TimingArgs,
    /// Data frames per feedback window; 1 selects stop-and-wait.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u8).range(1..=32))]
    pub window: u8,
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub retries: u32,
    /// Seconds after output drain/settle; allows recovery from corrupt long headers.
    #[arg(long, default_value_t = 40, value_parser = clap::value_parser!(u64).range(35..=300))]
    pub response_timeout: u64,
    /// Create a new CSV event log (the parent directory must exist).
    #[arg(long)]
    pub events: Option<PathBuf>,
}
#[derive(Debug, Args)]
pub struct ReceiveArgs {
    /// Exact output file; received filename metadata never selects a path.
    pub output: PathBuf,
    /// Receive an encoded WAV played by a phone; save on SHA-256 verification
    /// without Poll/ACK/Close or automatic retransmission.
    #[arg(long)]
    pub one_way: bool,
    /// Save selected microphone samples as a new 48 kHz mono PCM16 diagnostic WAV,
    /// including on timeout/Ctrl+C. Requires --one-way; never overwrites a file.
    #[arg(long, requires = "one_way")]
    pub capture_wav: Option<PathBuf>,
    #[command(flatten)]
    pub audio: AudioArgs,
    #[command(flatten)]
    pub timing: TimingArgs,
    /// Seconds without a relevant valid frame before aborting an incomplete session.
    #[arg(long, default_value_t = 900, value_parser = clap::value_parser!(u64).range(60..=86400))]
    pub idle_timeout: u64,
    /// After commit, answer repeated final polls until Close or this many seconds.
    #[arg(long, default_value_t = 400, value_parser = clap::value_parser!(u64).range(40..=86400))]
    pub linger: u64,
    #[arg(long)]
    pub events: Option<PathBuf>,
}

pub fn send(args: SendArgs) -> Result<()> {
    let input =
        File::open(&args.input).with_context(|| format!("cannot read {}", args.input.display()))?;
    if input.metadata()?.len() > MAX_FILE_SIZE {
        bail!("file exceeds the 16 MiB transfer sequence limit");
    }
    let mut data = Vec::new();
    input.take(MAX_FILE_SIZE + 1).read_to_end(&mut data)?;
    let name = args
        .input
        .file_name()
        .context("input needs a filename")?
        .to_string_lossy();
    // Fresh ID plus round/window correlation rejects old status. This is not authentication.
    let mut seed = Sha256::new();
    seed.update(&data);
    seed.update(std::process::id().to_be_bytes());
    seed.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .to_be_bytes(),
    );
    let id = u32::from_be_bytes(seed.finalize()[..4].try_into().unwrap());
    let mut sender = SendSession::new(id, &name, &data, args.window, args.retries)?;
    let timing = args.timing.timing(args.response_timeout);
    sender.set_timing(timing)?;
    let log = EventLog::open(args.events.as_deref())
        .context("cannot create event log; use a new path")?;
    let mut link = Link::new(
        Audio::open(&(&args.audio).into(), audio::cancellation()?)?,
        timing,
        log,
    )?;
    link.set_reporter(|message| println!("{message}"));
    println!(
        "Live TX: {} bytes; transfer ID {id:08X}; {} data packets; window {}",
        data.len(),
        sender.metadata().data_packets,
        args.window
    );
    let result = run_sender(&mut link, &mut sender);
    if let Err(error) = &result {
        sender.cancel();
        let _ = link.event("failure", &error.to_string());
    }
    link.summary(data.len() as u64, sender.is_complete())?;
    result?;
    println!(
        "Transfer success: receiver committed {} bytes and confirmed SHA-256 {}",
        data.len(),
        sender
            .metadata()
            .sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    Ok(())
}

pub fn receive(args: ReceiveArgs) -> Result<()> {
    let parent = args
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        bail!("destination directory does not exist: {}", parent.display());
    }
    if args.output.is_dir() {
        bail!("receive expects an exact output file, not a directory");
    }
    validate_receive_paths(
        &args.output,
        args.events.as_deref(),
        args.capture_wav.as_deref(),
    )?;
    let diagnostics = args
        .one_way
        .then(|| capture::InputDiagnostics::new(args.capture_wav.as_deref()))
        .transpose()?;
    let log = EventLog::open(args.events.as_deref())
        .context("cannot create event log; use a new path")?;
    let mut audio_options = AudioOptions::from(&args.audio);
    if args.one_way {
        // A phone receiver never sends feedback. Avoid opening an unrelated
        // speaker endpoint, which can disturb some shared WASAPI drivers.
        audio_options.output_enabled = false;
    }
    let mut link = Link::new(
        Audio::open(&audio_options, audio::cancellation()?)?,
        args.timing.timing(40),
        log,
    )?;
    link.set_reporter(|message| println!("{message}"));
    if args.one_way {
        link.input_diagnostics = diagnostics;
        if let Some(path) = &args.capture_wav {
            println!("Microphone diagnostic WAV -> {}", path.display());
        }
        println!("Listening for one-way audio -> {}", args.output.display());
        let result = one_way::receive(&mut link, args.idle_timeout * 1000, |file| {
            commit_file(&args.output, file).map_err(|e| e.to_string())
        });
        if let Err(error) = &result {
            let _ = link.event("failure", &error.to_string());
        }
        let captured = link.finish_input_diagnostics();
        if let Err(error) = &captured {
            eprintln!("Could not finalize microphone diagnostics: {error}");
        }
        if let Some(path) = &args.capture_wav
            && captured.is_ok()
        {
            println!("Microphone diagnostic WAV saved -> {}", path.display());
        }
        result?;
        captured?;
        return Ok(());
    }
    let mut session = ReceiveSession::default();
    println!(
        "Listening for an acoustic file transfer -> {}",
        args.output.display()
    );
    let result = run_receiver(
        &mut link,
        &mut session,
        args.idle_timeout * 1000,
        args.linger * 1000,
        |file| commit_file(&args.output, file).map_err(|e| e.to_string()),
    );
    if let Err(error) = &result {
        session.cancel();
        let _ = link.event("failure", &error.to_string());
        if session.is_committed() {
            eprintln!(
                "The verified destination was already saved; the audio completion exchange failed."
            );
        }
    }
    link.summary(
        session.progress().map_or(0, |p| p.buffered_bytes as u64),
        session.is_committed(),
    )?;
    result
}

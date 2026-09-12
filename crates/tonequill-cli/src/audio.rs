use anyhow::{Context, Result, bail};
use clap::Args;
use cpal::traits::{DeviceTrait, HostTrait};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tonequill_live::audio::{Audio, AudioOptions, AudioPort, InputEvent};
#[derive(Debug, Clone, Default, Args)]
pub struct AudioArgs {
    /// Input ID or current index printed by audio-devices; default: system microphone.
    #[arg(long)]
    pub input_device: Option<String>,
    /// Output ID or current index printed by audio-devices; default: system speakers.
    #[arg(long)]
    pub output_device: Option<String>,
    /// Zero-based microphone channel; mono TX is copied to every output channel.
    #[arg(long, default_value_t = 0)]
    pub input_channel: usize,
}

impl From<&AudioArgs> for AudioOptions {
    fn from(args: &AudioArgs) -> Self {
        Self {
            input_device: args.input_device.clone(),
            output_device: args.output_device.clone(),
            input_channel: args.input_channel,
            input_enabled: true,
            output_enabled: true,
        }
    }
}
pub fn devices() -> Result<()> {
    let host = cpal::default_host();
    println!("Audio host: {}", host.id().name());
    for input in [true, false] {
        println!("{} devices:", if input { "Input" } else { "Output" });
        let devices: Vec<_> = if input {
            host.input_devices()?.collect()
        } else {
            host.output_devices()?.collect()
        };
        let default = if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        };
        for (index, device) in devices.iter().enumerate() {
            let config = if input {
                device.default_input_config()
            } else {
                device.default_output_config()
            };
            println!(
                "  {index}: {device}{}; {}\n     ID: {}",
                if Some(device) == default.as_ref() {
                    " [default]"
                } else {
                    ""
                },
                config.map_or_else(|e| e.to_string(), |c| format!("{c:?}")),
                device
                    .id()
                    .map_or_else(|e| format!("unavailable ({e})"), |id| id.to_string())
            );
        }
    }
    Ok(())
}

pub fn cancellation() -> Result<Arc<AtomicBool>> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Release))
        .context("cannot install Ctrl+C handler")?;
    Ok(cancelled)
}

pub fn check(args: &AudioArgs, seconds: u64) -> Result<()> {
    if !(1..=60).contains(&seconds) {
        bail!("audio-check duration must be 1..60 seconds");
    }
    let mut audio = Audio::open(&args.into(), cancellation()?)?;
    let deadline = audio.now_ms() + seconds * 1000;
    let (mut samples, mut power, mut peak, mut gaps) = (0_u64, 0.0_f64, 0.0_f32, 0_u64);
    while audio.now_ms() < deadline {
        match audio.read(Duration::from_millis(50))? {
            InputEvent::Samples(chunk) => {
                for x in chunk {
                    samples += 1;
                    power += f64::from(x).powi(2);
                    peak = peak.max(x.abs());
                }
            }
            InputEvent::Gap | InputEvent::Discontinuity => gaps += 1,
            InputEvent::Idle => (),
        }
    }
    if samples == 0 {
        bail!("no microphone samples received");
    }
    println!(
        "Audio streams opened; captured samples: {samples}; RMS: {:.6}; peak: {peak:.6}; gaps: {gaps}",
        (power / samples as f64).sqrt()
    );
    Ok(())
}

//! CPAL device adapter. Modulation, decoding, files and waits run on the worker.
mod buffers;
use crate::{
    errors::AppError,
    events::{DeviceInfo, ErrorCode},
};
use anyhow::{Context, Result, bail};
use buffers::{BLOCK, Block, CAPACITY, Capture, PREFILL, Playback, Shared};
use cpal::{
    Device, FromSample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tonequill_core::config::SAMPLE_RATE;

#[derive(Debug, Clone)]
pub struct AudioOptions {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub input_channel: usize,
    pub input_enabled: bool,
    pub output_enabled: bool,
}
impl Default for AudioOptions {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            input_channel: 0,
            input_enabled: true,
            output_enabled: true,
        }
    }
}

pub enum InputEvent {
    Samples(Vec<f32>),
    /// The backend marked a discontinuity but still supplied the next buffer.
    /// Preserve delivered PCM and let frame CRC decide whether it is usable.
    Discontinuity,
    /// The bounded application queue overflowed and definitely lost a block.
    Gap,
    Idle,
}

/// Worker-side interface, also implemented by the virtual sample backend.
/// write streams bounded chunks; finish drains playback, including device latency.
pub trait AudioPort {
    fn now_ms(&self) -> u64;
    fn read(&mut self, timeout: Duration) -> Result<InputEvent>;
    fn set_listening(&mut self, enabled: bool);
    fn write(&mut self, samples: &[f32]) -> Result<()>;
    fn finish(&mut self) -> Result<()>;
    fn pause(&mut self, duration: Duration) -> Result<()>;
}

pub struct Audio {
    _input: Option<Stream>,
    _output: Option<Stream>,
    captured: Consumer<Block>,
    playback: Producer<Block>,
    shared: Arc<Shared>,
    cancelled: Arc<AtomicBool>,
    origin: Instant,
    observed_overruns: u64,
    observed_discontinuities: u64,
    ticket: u64,
    queued_blocks: usize,
    writing: bool,
    limit_ms: Option<u64>,
}

fn select_device(host: &cpal::Host, selector: Option<&str>, input: bool) -> Result<Device> {
    let device = if let Some(selector) = selector {
        let devices: Vec<_> = if input {
            host.input_devices()?.collect()
        } else {
            host.output_devices()?.collect()
        };
        if let Ok(index) = selector.parse::<usize>() {
            devices.into_iter().nth(index)
        } else {
            devices
                .into_iter()
                .find(|d| d.id().is_ok_and(|id| id.to_string() == selector))
        }
    } else if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    };
    device.with_context(|| {
        format!(
            "{} device not found; run audio-devices and copy its ID or index",
            if input { "input" } else { "output" }
        )
    })
}

fn choose_config(device: &Device, input: bool, channel: usize) -> Result<SupportedStreamConfig> {
    let ranges: Vec<_> = if input {
        device.supported_input_configs()?.collect()
    } else {
        device.supported_output_configs()?.collect()
    };
    select_config(ranges, channel)
}

/// Device discovery never opens an audio stream or records microphone samples.
pub fn list_devices() -> Result<Vec<DeviceInfo>> {
    let host = cpal::default_host();
    let mut result = Vec::new();
    for input in [true, false] {
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
        for device in devices {
            let config = choose_config(&device, input, 0);
            result.push(DeviceInfo {
                id: device.id()?.to_string(),
                name: device.to_string(),
                is_input: input,
                is_default: default.as_ref() == Some(&device),
                compatible: config.is_ok(),
                channels: config.as_ref().ok().map(|c| u32::from(c.channels())),
                detail: config.err().map(|error| error.to_string()),
            });
        }
    }
    Ok(result)
}

fn select_config(
    ranges: Vec<cpal::SupportedStreamConfigRange>,
    channel: usize,
) -> Result<SupportedStreamConfig> {
    let mut configs: Vec<_> = ranges
        .into_iter()
        .filter(|c| c.min_sample_rate() <= SAMPLE_RATE && c.max_sample_rate() >= SAMPLE_RATE)
        .filter(|c| usize::from(c.channels()) > channel && c.channels() <= 32)
        .filter(|c| {
            matches!(
                c.sample_format(),
                SampleFormat::F32 | SampleFormat::I16 | SampleFormat::I32 | SampleFormat::U16
            )
        })
        .map(|c| c.with_sample_rate(SAMPLE_RATE))
        .collect();
    configs.sort_by_key(|c| (c.sample_format() != SampleFormat::F32, c.channels()));
    configs.into_iter().next().context(AppError::new(ErrorCode::UnsupportedAudio, "no supported 48 kHz f32/i16/i32/u16 configuration with the requested channel; select another device/channel or configure the device to 48 kHz"))
}

impl Audio {
    pub fn open(args: &AudioOptions, cancelled: Arc<AtomicBool>) -> Result<Self> {
        Self::open_limited(args, cancelled, None)
    }
    pub fn open_limited(
        args: &AudioOptions,
        cancelled: Arc<AtomicBool>,
        limit_ms: Option<u64>,
    ) -> Result<Self> {
        anyhow::ensure!(
            !cancelled.load(Ordering::Acquire),
            "session cancelled before opening audio"
        );
        anyhow::ensure!(
            args.input_enabled || args.output_enabled,
            "at least one audio direction must be enabled"
        );
        let host = cpal::default_host();
        let input = args
            .input_enabled
            .then(|| {
                select_device(&host, args.input_device.as_deref(), true).context(AppError::new(
                    ErrorCode::DeviceUnavailable,
                    "Microphone is unavailable",
                ))
            })
            .transpose()?;
        let output =
            args.output_enabled
                .then(|| {
                    select_device(&host, args.output_device.as_deref(), false).context(
                        AppError::new(ErrorCode::DeviceUnavailable, "Speaker is unavailable"),
                    )
                })
                .transpose()?;
        let (capture_tx, captured) = RingBuffer::new(CAPACITY);
        let (playback, playback_rx) = RingBuffer::new(CAPACITY);
        let shared = Arc::new(Shared::default());
        shared.epoch.store(1, Ordering::Release);
        let origin = Instant::now();
        let input = input
            .map(|input| {
                let input_config = choose_config(&input, true, args.input_channel)?;
                let capture = Capture {
                    queue: capture_tx,
                    shared: shared.clone(),
                    channel: args.input_channel,
                };
                let stream = match input_config.sample_format() {
                    SampleFormat::F32 => {
                        input_stream::<f32>(&input, input_config.into(), capture, origin)?
                    }
                    SampleFormat::I16 => {
                        input_stream::<i16>(&input, input_config.into(), capture, origin)?
                    }
                    SampleFormat::I32 => {
                        input_stream::<i32>(&input, input_config.into(), capture, origin)?
                    }
                    SampleFormat::U16 => {
                        input_stream::<u16>(&input, input_config.into(), capture, origin)?
                    }
                    _ => bail!("unsupported input format"),
                };
                Ok::<_, anyhow::Error>(stream)
            })
            .transpose()?;
        let output = output
            .map(|output| {
                let output_config = choose_config(&output, false, 0)?;
                let renderer = Playback::new(playback_rx, shared.clone());
                let stream = match output_config.sample_format() {
                    SampleFormat::F32 => {
                        output_stream::<f32>(&output, output_config.into(), renderer, origin)?
                    }
                    SampleFormat::I16 => {
                        output_stream::<i16>(&output, output_config.into(), renderer, origin)?
                    }
                    SampleFormat::I32 => {
                        output_stream::<i32>(&output, output_config.into(), renderer, origin)?
                    }
                    SampleFormat::U16 => {
                        output_stream::<u16>(&output, output_config.into(), renderer, origin)?
                    }
                    _ => bail!("unsupported output format"),
                };
                Ok::<_, anyhow::Error>(stream)
            })
            .transpose()?;
        anyhow::ensure!(
            !cancelled.load(Ordering::Acquire),
            "session cancelled before starting audio"
        );
        if let Some(input) = &input {
            input.play().context("cannot start microphone stream")?;
        }
        if let Some(output) = &output {
            output.play().context("cannot start speaker stream")?;
        }
        Ok(Self {
            _input: input,
            _output: output,
            captured,
            playback,
            shared,
            cancelled,
            origin,
            observed_overruns: 0,
            observed_discontinuities: 0,
            ticket: 0,
            queued_blocks: 0,
            writing: false,
            limit_ms,
        })
    }

    fn health(&self) -> Result<()> {
        anyhow::ensure!(
            self.limit_ms.is_none_or(|limit| self.now_ms() < limit),
            AppError::new(ErrorCode::Timeout, "Session duration limit reached")
        );
        if self.cancelled.load(Ordering::Acquire) {
            bail!("transfer cancelled (Ctrl+C)");
        }
        if self.shared.stream_error.load(Ordering::Acquire) {
            if let Some((direction, error)) = self.shared.backend_error.get() {
                return Err(AppError::new(if error.kind() == cpal::ErrorKind::PermissionDenied { ErrorCode::PermissionDenied } else { ErrorCode::AudioInterrupted }, format!("{direction} audio stream failed ({:?}): {error}; rerun audio-devices and audio-check", error.kind())).into());
            }
            return Err(AppError::new(
                ErrorCode::AudioInterrupted,
                "speaker playback timestamp exceeds the supported 5-second latency",
            )
            .into());
        }
        if self.shared.underflow.load(Ordering::Acquire) {
            return Err(AppError::new(ErrorCode::AudioInterrupted,
                "speaker buffer underrun interrupted the waveform; close other CPU-heavy programs and retry in --release mode"
            ).into());
        }
        if self._input.is_some()
            && self
                .now_ms()
                .saturating_sub(self.shared.input_at_ms.load(Ordering::Acquire))
                > 5000
        {
            return Err(AppError::new(
                ErrorCode::AudioInterrupted,
                "microphone callbacks stopped for more than 5 seconds; check the input device",
            )
            .into());
        }
        Ok(())
    }

    fn enqueue(&mut self, mut block: Block) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.health()?;
            match self.playback.push(block) {
                Ok(()) => return Ok(()),
                Err(rtrb::PushError::Full(returned)) => block = returned,
            }
            if Instant::now() >= deadline {
                bail!("speaker callbacks stalled while queueing audio");
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}

impl AudioPort for Audio {
    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }
    fn read(&mut self, timeout: Duration) -> Result<InputEvent> {
        anyhow::ensure!(self._input.is_some(), "microphone stream is not open");
        let deadline = Instant::now() + timeout;
        loop {
            self.health()?;
            if let Some(event) = capture_event(
                &mut self.captured,
                &self.shared,
                &mut self.observed_overruns,
                &mut self.observed_discontinuities,
            ) {
                return Ok(event);
            }
            if Instant::now() >= deadline {
                return Ok(InputEvent::Idle);
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn set_listening(&mut self, enabled: bool) {
        let previous = self.shared.epoch.load(Ordering::Acquire);
        // In-flight callbacks from before a flush cannot appear as fresh capture.
        let next = previous
            + if !previous.is_multiple_of(2) == enabled {
                2
            } else {
                1
            };
        self.shared.epoch.store(next, Ordering::Release);
        while self.captured.pop().is_ok() {}
        self.observed_overruns = self.shared.overruns.load(Ordering::Acquire);
        self.observed_discontinuities = self.shared.discontinuities.load(Ordering::Acquire);
    }
    fn write(&mut self, samples: &[f32]) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        anyhow::ensure!(self._output.is_some(), "speaker stream is not open");
        self.writing = true;
        for part in samples.chunks(BLOCK) {
            let mut block = Block {
                len: part.len(),
                ..Default::default()
            };
            block.samples[..part.len()].copy_from_slice(part);
            self.enqueue(block)?;
            self.queued_blocks += 1;
            if self.queued_blocks == PREFILL {
                self.shared.active.store(true, Ordering::Release);
            }
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        if !self.writing {
            return Ok(());
        }
        self.ticket += 1;
        self.enqueue(Block {
            tag: self.ticket,
            ..Default::default()
        })?;
        // A short burst may not have reached the normal prefill threshold.
        if self.queued_blocks < PREFILL {
            self.shared.active.store(true, Ordering::Release);
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.shared.played.load(Ordering::Acquire) < self.ticket {
            self.health()?;
            if Instant::now() >= deadline {
                bail!("speaker callbacks stalled before end of playback");
            }
            thread::sleep(Duration::from_millis(2));
        }
        // Callback consumption precedes DAC playback: honor its timestamp.
        let physical_end = self.shared.finished_at_us.load(Ordering::Acquire);
        let remaining = physical_end.saturating_sub(self.origin.elapsed().as_micros() as u64);
        if remaining > 0 {
            self.pause(Duration::from_micros(remaining))?;
        }
        self.queued_blocks = 0;
        self.writing = false;
        self.health()
    }
    fn pause(&mut self, duration: Duration) -> Result<()> {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.health()?;
            thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10)),
            );
        }
        self.health()
    }
}

fn capture_event(
    captured: &mut Consumer<Block>,
    shared: &Shared,
    observed_overruns: &mut u64,
    observed_discontinuities: &mut u64,
) -> Option<InputEvent> {
    let overruns = shared.overruns.load(Ordering::Acquire);
    if overruns != *observed_overruns {
        *observed_overruns = overruns;
        // A full application queue proves that at least one block is absent.
        // Discard the stale backlog and restart acquisition at a known boundary.
        while captured.pop().is_ok() {}
        *observed_discontinuities = shared.discontinuities.load(Ordering::Acquire);
        return Some(InputEvent::Gap);
    }
    let discontinuities = shared.discontinuities.load(Ordering::Acquire);
    if discontinuities != *observed_discontinuities {
        *observed_discontinuities = discontinuities;
        return Some(InputEvent::Discontinuity);
    }
    loop {
        let block = captured.pop().ok()?;
        let epoch = shared.epoch.load(Ordering::Acquire);
        if block.tag == epoch && !epoch.is_multiple_of(2) {
            return Some(InputEvent::Samples(block.samples[..block.len].to_vec()));
        }
    }
}

fn input_error(shared: &Shared, error: cpal::Error) {
    if error.kind() == cpal::ErrorKind::Xrun {
        // WASAPI emits this before invoking the data callback for the flagged
        // buffer. Keep that PCM: CRC still prevents accepting damaged frames.
        shared.discontinuities.fetch_add(1, Ordering::Release);
    } else {
        stream_error(shared, "microphone", error);
    }
}

fn stream_error(shared: &Shared, direction: &'static str, error: cpal::Error) {
    // Keep the first cause, including backend details. This is a terminal
    // error callback; ordinary sample callbacks do not allocate or lock.
    let _ = shared.backend_error.set((direction, error));
    shared.stream_error.store(true, Ordering::Release);
}

fn input_stream<T>(
    device: &Device,
    config: StreamConfig,
    mut capture: Capture,
    origin: Instant,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let errors = capture.shared.clone();
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            capture
                .shared
                .input_at_ms
                .store(origin.elapsed().as_millis() as u64, Ordering::Release);
            capture.process(data, channels);
        },
        move |error| input_error(&errors, error),
        Some(Duration::from_secs(5)),
    )?)
}

fn output_stream<T>(
    device: &Device,
    config: StreamConfig,
    mut playback: Playback,
    origin: Instant,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let errors = playback.shared.clone();
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], info| {
            if let Some((ticket, offset)) = playback.process(data, channels) {
                let timestamp = info.timestamp();
                let latency = timestamp.playback.duration_since(timestamp.callback);
                if latency > Duration::from_secs(5) {
                    playback.shared.stream_error.store(true, Ordering::Release);
                    return;
                }
                let end = origin.elapsed()
                    + latency
                    + Duration::from_secs_f64(offset as f64 / f64::from(SAMPLE_RATE));
                playback
                    .shared
                    .finished_at_us
                    .store(end.as_micros() as u64, Ordering::Release);
                playback.shared.played.store(ticket, Ordering::Release);
            }
        },
        move |error| stream_error(&errors, "speaker", error),
        Some(Duration::from_secs(5)),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microphone_xrun_preserves_delivered_audio_and_reports_discontinuity() {
        let shared = Arc::new(Shared::default());
        shared.epoch.store(1, Ordering::Release);
        let (queue, mut captured) = RingBuffer::new(4);
        let mut capture = Capture {
            queue,
            shared: shared.clone(),
            channel: 0,
        };
        let mut observed_overruns = 0;
        let mut observed_discontinuities = 0;
        capture.process(&[0.1_f32; 480], 1);
        input_error(&shared, cpal::ErrorKind::Xrun.into());
        capture.process(&[0.2_f32; 480], 1);
        assert!(!shared.stream_error.load(Ordering::Acquire));
        assert!(shared.backend_error.get().is_none());
        assert!(matches!(
            capture_event(
                &mut captured,
                &shared,
                &mut observed_overruns,
                &mut observed_discontinuities
            ),
            Some(InputEvent::Discontinuity)
        ));
        assert_eq!(observed_discontinuities, 1);
        let Some(InputEvent::Samples(samples)) = capture_event(
            &mut captured,
            &shared,
            &mut observed_overruns,
            &mut observed_discontinuities,
        ) else {
            panic!("audio queued before the discontinuity must be retained");
        };
        assert_eq!(samples, vec![0.1; 480]);
        let Some(InputEvent::Samples(samples)) = capture_event(
            &mut captured,
            &shared,
            &mut observed_overruns,
            &mut observed_discontinuities,
        ) else {
            panic!("the flagged WASAPI buffer must be retained");
        };
        assert_eq!(samples, vec![0.2; 480]);
        input_error(&shared, cpal::ErrorKind::Xrun.into());
        assert!(matches!(
            capture_event(
                &mut captured,
                &shared,
                &mut observed_overruns,
                &mut observed_discontinuities
            ),
            Some(InputEvent::Discontinuity)
        ));
        assert_eq!(observed_discontinuities, 2);
    }

    #[test]
    fn fatal_backend_errors_keep_the_first_cause_and_output_xruns_still_fail() {
        for kind in [
            cpal::ErrorKind::DeviceNotAvailable,
            cpal::ErrorKind::StreamInvalidated,
            cpal::ErrorKind::PermissionDenied,
            cpal::ErrorKind::BackendError,
        ] {
            let shared = Shared::default();
            input_error(&shared, cpal::Error::with_message(kind, "driver detail"));
            input_error(&shared, cpal::ErrorKind::Xrun.into());
            stream_error(
                &shared,
                "speaker",
                cpal::ErrorKind::DeviceNotAvailable.into(),
            );
            assert!(shared.stream_error.load(Ordering::Acquire));
            let (direction, error) = shared.backend_error.get().unwrap();
            assert_eq!(*direction, "microphone");
            assert_eq!(error.kind(), kind);
            assert_eq!(error.message(), Some("driver detail"));
        }
        let shared = Shared::default();
        stream_error(&shared, "speaker", cpal::ErrorKind::Xrun.into());
        assert!(shared.stream_error.load(Ordering::Acquire));
        assert_eq!(shared.overruns.load(Ordering::Acquire), 0);
        assert_eq!(shared.backend_error.get().unwrap().0, "speaker");
    }

    #[test]
    fn configuration_requires_exact_48khz_and_the_selected_input_channel() {
        let range = |channels, rate, format| {
            cpal::SupportedStreamConfigRange::new(
                channels,
                rate,
                rate,
                cpal::SupportedBufferSize::Unknown,
                format,
            )
        };
        let ranges = vec![
            range(1, 44100, SampleFormat::F32),
            range(1, 48000, SampleFormat::F32),
            range(2, 48000, SampleFormat::I16),
        ];
        assert_eq!(select_config(ranges.clone(), 0).unwrap().channels(), 1);
        let stereo = select_config(ranges.clone(), 1).unwrap();
        assert_eq!(stereo.channels(), 2);
        assert_eq!(stereo.sample_format(), SampleFormat::I16);
        assert_eq!(stereo.sample_rate(), SAMPLE_RATE);
        assert!(select_config(ranges, 2).is_err());
        assert!(select_config(vec![range(2, 44100, SampleFormat::F32)], 0).is_err());
    }
}

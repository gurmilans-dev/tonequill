//! Optional diagnostic PCM capture of the selected input channel, before DSP.
use anyhow::{Context, Result, bail};
use std::{
    fs::{File, OpenOptions},
    io::BufWriter,
    path::Path,
};
use tonequill_core::config::SAMPLE_RATE;

pub struct InputDiagnostics {
    writer: Option<hound::WavWriter<BufWriter<File>>>,
    pub samples: u64,
    count: u64,
    squares: f64,
    peak: f32,
    clipped: u64,
    last_report: u64,
    pub interval_ms: u64,
}

impl InputDiagnostics {
    pub fn new(path: Option<&Path>) -> Result<Self> {
        let writer = path
            .map(|path| {
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .with_context(|| {
                        format!(
                            "cannot create microphone capture {}; use a new path",
                            path.display()
                        )
                    })?;
                hound::WavWriter::new(
                    BufWriter::new(file),
                    hound::WavSpec {
                        channels: 1,
                        sample_rate: SAMPLE_RATE,
                        bits_per_sample: 16,
                        sample_format: hound::SampleFormat::Int,
                    },
                )
                .map_err(anyhow::Error::from)
            })
            .transpose()?;
        Ok(Self {
            writer,
            samples: 0,
            count: 0,
            squares: 0.0,
            peak: 0.0,
            clipped: 0,
            last_report: 0,
            interval_ms: 2000,
        })
    }

    pub fn observe(&mut self, samples: &[f32]) -> Result<()> {
        if samples.iter().any(|s| !s.is_finite()) {
            bail!("microphone supplied non-finite samples");
        }
        for &sample in samples {
            self.squares += f64::from(sample).powi(2);
            self.peak = self.peak.max(sample.abs());
            self.clipped += u64::from(sample.abs() >= 1.0);
            if let Some(writer) = &mut self.writer {
                writer.write_sample((sample.clamp(-1.0, 1.0) * 32767.0) as i16)?;
            }
        }
        self.samples += samples.len() as u64;
        self.count += samples.len() as u64;
        Ok(())
    }

    pub fn report(&mut self, now_ms: u64, force: bool) -> Result<Option<String>> {
        Ok(self
            .report_with_metrics(now_ms, force)?
            .map(|(text, _)| text))
    }

    pub fn report_with_metrics(
        &mut self,
        now_ms: u64,
        force: bool,
    ) -> Result<Option<(String, crate::events::SignalMetrics)>> {
        if !force && now_ms.saturating_sub(self.last_report) < self.interval_ms {
            return Ok(None);
        }
        let rms = (self.squares / self.count.max(1) as f64).sqrt();
        let metrics = crate::events::SignalMetrics {
            samples: self.samples as f64,
            rms,
            peak: self.peak,
            clipped_samples: self.clipped.min(u32::MAX as u64) as u32,
            gaps: 0,
        };
        let text = format!(
            "samples={}; audio_s={:.3}; window_samples={}; RMS={rms:.6}; peak={:.6}; clipped={}",
            self.samples,
            self.samples as f64 / SAMPLE_RATE as f64,
            self.count,
            self.peak,
            self.clipped
        );
        if let Some(writer) = &mut self.writer {
            writer.flush()?;
        }
        self.count = 0;
        self.squares = 0.0;
        self.peak = 0.0;
        self.last_report = now_ms;
        Ok(Some((text, metrics)))
    }

    pub fn finish(&mut self) -> Result<()> {
        if let Some(writer) = self.writer.take() {
            writer.finalize()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "tonequill-{label}-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn captured_pcm_and_live_levels_remain_readable_after_flush_and_finalize() {
        let path = path("capture");
        let mut capture = InputDiagnostics::new(Some(&path)).unwrap();
        capture.observe(&[0.0, 0.25, -0.5, 1.25]).unwrap();
        assert!(capture.report(1999, false).unwrap().is_none());
        let report = capture.report(2000, false).unwrap().unwrap();
        assert!(report.contains("window_samples=4"));
        assert!(report.contains("peak=1.250000; clipped=1"));
        assert!(capture.observe(&[f32::NAN]).is_err());
        assert_eq!(capture.samples, 4);
        assert_eq!(hound::WavReader::open(&path).unwrap().duration(), 4);
        capture.finish().unwrap();
        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, 48000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(
            reader
                .samples::<i16>()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            [0, 8191, -16383, 32767]
        );
        drop(reader);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn capture_never_overwrites_existing_evidence() {
        let path = path("existing");
        std::fs::write(&path, b"preserve evidence").unwrap();
        assert!(InputDiagnostics::new(Some(&path)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"preserve evidence");
        std::fs::remove_file(path).unwrap();
    }
}

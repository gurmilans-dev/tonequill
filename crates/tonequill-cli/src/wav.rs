use anyhow::{Context, Result, bail};
use std::{fs::File, io::BufReader, path::Path};
use tonequill_core::config::SAMPLE_RATE;
use tonequill_core::{
    protocol::packet::Packet,
    receiver::{FrameAttempt, FrameScanner},
    transfer::waveform::{EDGE_PADDING_SAMPLES, INTER_FRAME_SAMPLES, frame_samples},
};
pub fn write_wav(path: &Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("cannot create WAV file: {}", path.display()))?;

    for &sample in samples {
        let sample = sample.clamp(-1.0, 1.0);

        let pcm = (sample * i16::MAX as f32) as i16;

        writer.write_sample(pcm)?;
    }

    writer.finalize()?;

    Ok(())
}
pub fn open_wav(path: &Path) -> Result<hound::WavReader<BufReader<File>>> {
    let reader = hound::WavReader::open(path)
        .with_context(|| format!("cannot open WAV file: {}", path.display()))?;

    let spec = reader.spec();

    if spec.channels != 1 {
        bail!("expected mono WAV, found {} channels", spec.channels);
    }

    if spec.sample_rate != SAMPLE_RATE {
        bail!(
            "expected sample rate {} Hz, found {} Hz",
            SAMPLE_RATE,
            spec.sample_rate
        );
    }

    if spec.bits_per_sample != 16 {
        bail!(
            "expected 16-bit PCM WAV, found {} bits",
            spec.bits_per_sample
        );
    }

    if spec.sample_format != hound::SampleFormat::Int {
        bail!("expected integer PCM WAV");
    }

    Ok(reader)
}

pub fn read_wav(path: &Path) -> Result<Vec<f32>> {
    let mut reader = open_wav(path)?;
    let mut samples = Vec::new();

    for sample in reader.samples::<i16>() {
        let sample = sample?;

        samples.push(sample as f32 / i16::MAX as f32);
    }

    Ok(samples)
}

pub fn write_transfer(path: &Path, packets: impl Iterator<Item = Packet>) -> Result<()> {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .with_context(|| format!("cannot create WAV file: {}", path.display()))?;
    for _ in 0..EDGE_PADDING_SAMPLES {
        writer.write_sample(0_i16)?;
    }
    let mut packets = packets.peekable();
    while let Some(packet) = packets.next() {
        for sample in frame_samples(&packet)? {
            writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        if packets.peek().is_some() {
            for _ in 0..INTER_FRAME_SAMPLES {
                writer.write_sample(0_i16)?;
            }
        }
    }
    for _ in 0..EDGE_PADDING_SAMPLES {
        writer.write_sample(0_i16)?;
    }
    writer.finalize()?;
    Ok(())
}

pub fn visit_frames(path: &Path, mut visit: impl FnMut(FrameAttempt) -> Result<()>) -> Result<()> {
    let mut reader = open_wav(path)?;
    let mut samples = reader.samples::<i16>();
    let mut scanner = FrameScanner::default();
    loop {
        let chunk: Vec<f32> = samples
            .by_ref()
            .take(65536)
            .map(|s| s.map(|s| s as f32 / i16::MAX as f32))
            .collect::<Result<_, _>>()?;
        if chunk.is_empty() {
            break;
        }
        for attempt in scanner.push(&chunk)? {
            visit(attempt)?;
        }
    }
    for attempt in scanner.finish()? {
        visit(attempt)?;
    }
    Ok(())
}

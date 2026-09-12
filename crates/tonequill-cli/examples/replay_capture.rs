//! Replay diagnostic PCM through the incremental receiver without audio devices.
//! Usage: replay_capture <mono-48k-PCM16.wav> [padding] [gap-sample ...]
//! Gaps use original WAV coordinates; samples are never filled in or repaired.
use anyhow::{Result, ensure};
use sha2::Digest;
use std::time::{Duration, Instant};
use tonequill_core::{receiver::LiveDecoder, transfer::Reassembler};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() >= 2,
        "usage: replay_capture <wav> [padding] [gap-sample ...]"
    );
    let padding: usize = args.get(2).map_or(Ok(0), |s| s.parse())?;
    let mut gaps: Vec<usize> = args[3.min(args.len())..]
        .iter()
        .map(|s| s.parse())
        .collect::<Result<_, _>>()?;
    let mut wav = hound::WavReader::open(&args[1])?;
    let spec = wav.spec();
    ensure!(
        spec.channels == 1
            && spec.sample_rate == 48000
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "expected mono 48 kHz PCM16"
    );
    let mut samples = vec![0.0; padding];
    samples.extend(
        wav.samples::<i16>()
            .map(|s| s.map(|s| s as f32 / 32767.0))
            .collect::<Result<Vec<_>, _>>()?,
    );
    ensure!(
        gaps.iter().all(|&gap| gap <= samples.len() - padding),
        "gap beyond recording"
    );
    gaps.iter_mut().for_each(|gap| *gap += padding);
    gaps.sort_unstable();
    gaps.dedup();
    let mut gaps = gaps.into_iter().peekable();
    let mut decoder = LiveDecoder::default();
    let mut reassembler = Reassembler::default();
    let mut position = 0;
    let mut max_push = Duration::ZERO;
    let mut max_backlog = 0.0_f64;
    let mut work_clock = 0.0_f64;
    let start = Instant::now();
    let (mut valid, mut rejected) = (0, 0);
    while position < samples.len() {
        while gaps.peek() == Some(&position) {
            decoder.reset();
            gaps.next();
        }
        let end = (position + 480)
            .min(samples.len())
            .min(gaps.peek().copied().unwrap_or(usize::MAX));
        let step = Instant::now();
        let attempts = decoder.push(&samples[position..end])?;
        let elapsed = step.elapsed();
        max_push = max_push.max(elapsed);
        // Simulate an otherwise idle worker receiving samples at 48 kHz. This
        // measures CPU backlog, not backend scheduling or actual device gaps.
        let arrival = end as f64 / 48000.0;
        work_clock = work_clock.max(arrival) + elapsed.as_secs_f64();
        max_backlog = max_backlog.max(work_clock - arrival);
        for attempt in attempts {
            match attempt.result {
                Ok(packet) => {
                    valid += 1;
                    println!(
                        "frame sample={end} onset={} id={:08X} flags={:02X} sequence={} bytes={}",
                        attempt.acquisition.start_sample,
                        packet.transfer_id,
                        packet.flags,
                        packet.sequence,
                        packet.payload.len()
                    );
                    reassembler.push(packet)?;
                }
                Err(error) => {
                    rejected += 1;
                    println!(
                        "rejected sample={end} onset={} error={error}",
                        attempt.acquisition.start_sample
                    );
                }
            }
        }
        position = end;
    }
    // No flush: EOF must not supply an advantage over receive --one-way.
    for id in reassembler.transfer_ids() {
        match reassembler.finish(id) {
            Ok(file) => println!(
                "verified id={id:08X} bytes={} sha256={}",
                file.bytes.len(),
                sha2::Sha256::digest(&file.bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ),
            Err(error) => println!("incomplete id={id:08X} error={error}"),
        }
    }
    println!(
        "summary valid={valid} rejected={rejected} audio_s={:.6} cpu_s={:.6} max_push_s={:.6} max_backlog_s={max_backlog:.6}",
        samples.len() as f64 / 48000.0,
        start.elapsed().as_secs_f64(),
        max_push.as_secs_f64()
    );
    Ok(())
}

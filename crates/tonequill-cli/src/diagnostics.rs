use crate::wav::read_wav;
use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};
use tonequill_core::{
    protocol::{bitstream::bytes_to_bits, framing::encode_frame},
    receiver::{self, ReceiverOptions, compare_bits, inspect},
};

pub fn diagnose(
    reference: &Path,
    recording: &Path,
    limit: usize,
    csv: Option<&Path>,
    options: ReceiverOptions,
) -> Result<()> {
    let reference_packet = receiver::receive(&read_wav(reference)?)
        .context("reference WAV must contain a CRC-valid Tonequill frame")?;
    if reference_packet.flags != 0 {
        bail!(
            "diagnose compares a legacy PHY packet; use encode --legacy for a single-frame reference, or decode for complete transfer verification"
        );
    }
    let expected = bytes_to_bits(&encode_frame(&reference_packet)?);
    let attempts = inspect(&read_wav(recording)?, options)?;
    let attempt = attempts
        .iter()
        .find(|a| a.result.is_ok())
        .unwrap_or(&attempts[0]);
    let acq = &attempt.acquisition;
    let comparison = compare_bits(&expected, &attempt.bits());
    println!(
        "candidates: {}; start: {:.2} samples; acquisition preamble: {}/64; sync: {}/16",
        attempts.len(),
        acq.start_sample,
        acq.preamble_matches,
        acq.sync_matches
    );
    println!(
        "acquisition quality: {:.5}; tone concentration: {:.5}",
        acq.quality, acq.tone_concentration
    );
    println!(
        "carrier RMS powers: 1200={:.6e}, 2200={:.6e}; E1/E0 threshold after preamble: {:.5}",
        acq.carrier_power[0], acq.carrier_power[1], acq.decision_ratio
    );
    println!(
        "preamble decision errors: power-ratio prior={}, fitted={}",
        acq.power_ratio_training_errors, acq.decision_training_errors
    );
    println!(
        "training threshold profile: {:.5?}",
        acq.preamble_decision_ratios
    );
    println!(
        "clock estimated: {}; initial/final samples per symbol: {:.6}/{:.6}; initial clock: {:.1} ppm",
        acq.clock_estimated,
        acq.samples_per_symbol,
        attempt.final_samples_per_symbol,
        (acq.samples_per_symbol / 480.0 - 1.0) * 1e6
    );
    println!(
        "reference bits: {}; decoded bits: {}; compared bits: {}; errors: {}; missing bits: {}; BER: {}",
        expected.len(),
        attempt.symbols.len(),
        comparison.compared_bits,
        comparison.bit_errors,
        comparison.missing_bits,
        comparison
            .ber()
            .map_or("n/a".to_owned(), |v| format!("{v:.8}"))
    );
    match &attempt.result {
        Ok(packet) => println!(
            "CRC32: OK; reference packet match: {}",
            *packet == reference_packet
        ),
        Err(error) => println!("frame validation: FAILED ({error})"),
    }
    let mut interesting: Vec<_> = attempt
        .symbols
        .iter()
        .filter(|s| s.confidence < 0.5 || expected.get(s.index).is_some_and(|&b| b != s.bit))
        .collect();
    interesting.sort_by(|a, b| a.confidence.total_cmp(&b.confidence));
    println!(
        "low-confidence/error symbols: {} (showing at most {limit})",
        interesting.len()
    );
    for s in interesting.iter().take(limit) {
        println!(
            "bit {:4}: TX={:?} RX={} confidence={:.4} P0={:.4e} P1={:.4e} center={:.2}",
            s.index,
            expected.get(s.index).map(|&b| u8::from(b)),
            u8::from(s.bit),
            s.confidence,
            s.carrier_power[0],
            s.carrier_power[1],
            s.center_sample
        );
    }
    if let Some(path) = csv {
        let mut writer = BufWriter::new(File::create(path)?);
        writeln!(
            writer,
            "bit,center_sample,reference,received,power_1200,power_2200,soft_value,confidence,timing_correction,decision_ratio"
        )?;
        for s in &attempt.symbols {
            let reference = expected
                .get(s.index)
                .map_or(String::new(), |&b| u8::from(b).to_string());
            writeln!(
                writer,
                "{},{:.6},{},{},{:.9e},{:.9e},{:.8},{:.8},{:.6},{:.8}",
                s.index,
                s.center_sample,
                reference,
                u8::from(s.bit),
                s.carrier_power[0],
                s.carrier_power[1],
                s.soft_value,
                s.confidence,
                s.timing_correction,
                s.decision_ratio
            )?;
        }
        writer.flush()?;
    }
    if !attempt
        .result
        .as_ref()
        .is_ok_and(|packet| *packet == reference_packet)
    {
        bail!("recording did not recover the reference packet");
    }
    Ok(())
}

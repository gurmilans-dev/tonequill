//! Per-candidate diagnostics. Reference bits are used only after demodulation;
//! they never influence acquisition, decisions, CRC or file reassembly.
use crate::wav::read_wav;
use anyhow::{Context, Result, bail};
use clap::Args;
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};
use tonequill_core::{
    config::SAMPLE_RATE,
    protocol::{
        bitstream::{bits_to_bytes, bytes_to_bits},
        framing::{PREFIX_SIZE, encode_frame},
        packet::Packet,
    },
    receiver::{FrameAttempt, ReceiverOptions, compare_bits, probe_energy_symbols, scan},
};

#[derive(Debug, Args)]
pub struct DiagnoseFramesArgs {
    /// Clean WAV containing one or more CRC-valid reference frames.
    reference: PathBuf,
    recording: PathBuf,
    #[arg(long, default_value_t = 20)]
    limit: usize,
    #[arg(long)]
    symbols_csv: Option<PathBuf>,
    #[arg(long)]
    frames_csv: Option<PathBuf>,
    #[arg(long)]
    no_calibration: bool,
    #[arg(long)]
    no_clock: bool,
    #[arg(long)]
    no_tracking: bool,
    #[arg(long)]
    full_symbols: bool,
    #[arg(long)]
    power_ratio_threshold: bool,
    /// Original energy receiver, including original calibrated acquisition.
    #[arg(long)]
    no_equalizer: bool,
    /// Inspect only this candidate (zero based).
    #[arg(long)]
    candidate: Option<usize>,
    /// Explicit diagnostic comparison; never used by the receiver.
    #[arg(long, requires = "candidate")]
    reference_frame: Option<usize>,
    /// Continue the energy receiver to the selected reference LENGTH after a
    /// bad prefix; exports measurements only, never declares reception success.
    #[arg(long, requires_all = ["candidate", "reference_frame"])]
    probe_energy: bool,
}

struct Reference {
    packet: Packet,
    bytes: Vec<u8>,
    bits: Vec<bool>,
}

fn csv(path: &Option<PathBuf>, header: &str) -> Result<Option<BufWriter<File>>> {
    path.as_ref()
        .map(|path| {
            let mut writer = BufWriter::new(File::create(path)?);
            writeln!(writer, "{header}")?;
            Ok(writer)
        })
        .transpose()
}

/// A failed CRC never makes a header trustworthy. An exact unique 10-byte
/// header match merely labels a *diagnostic* reference comparison. If it is
/// ambiguous/corrupt, print all alternatives instead of choosing by frame order.
fn reference_index(bytes: &[u8], references: &[Reference]) -> Option<usize> {
    let header = bytes.get(10..PREFIX_SIZE)?;
    let mut matched = references
        .iter()
        .enumerate()
        .filter(|(_, r)| &r.bytes[10..PREFIX_SIZE] == header);
    let index = matched.next()?.0;
    matched.next().is_none().then_some(index)
}

pub fn run(args: DiagnoseFramesArgs) -> Result<()> {
    let options = ReceiverOptions {
        calibrate_carriers: !args.no_calibration,
        estimate_clock: !args.no_clock,
        track_timing: !args.no_tracking,
        full_symbol_decisions: args.full_symbols,
        train_decision_threshold: !args.power_ratio_threshold,
        equalize: !args.no_equalizer,
    };
    let references: Vec<_> = scan(&read_wav(&args.reference)?, ReceiverOptions::default())?
        .into_iter()
        .map(|a| {
            let packet = a.result.context("reference contains a failed candidate")?;
            let bytes = encode_frame(&packet)?;
            let bits = bytes_to_bits(&bytes);
            Ok(Reference {
                packet,
                bytes,
                bits,
            })
        })
        .collect::<Result<_>>()?;
    if references.is_empty() {
        bail!("reference WAV contains no CRC-valid frames");
    }
    if args.reference_frame.is_some_and(|i| i >= references.len()) {
        bail!("reference-frame index out of range");
    }
    for (index, reference) in references.iter().enumerate() {
        let transitions = reference.bits.windows(2).filter(|w| w[0] != w[1]).count();
        println!(
            "TX reference {index}: kind=0x{:02X}; id={:08X}; sequence={}; payload_bytes={}; frame_bytes={}; bits={}; modulation_s={:.3}; transitions={transitions}",
            reference.packet.flags,
            reference.packet.transfer_id,
            reference.packet.sequence,
            reference.packet.payload.len(),
            reference.bytes.len(),
            reference.bits.len(),
            reference.bits.len() as f64 / 100.0
        );
    }
    let mut symbols = csv(
        &args.symbols_csv,
        "candidate,reference_frame,bit,center_sample,reference,received,power_1200,power_2200,soft_value,confidence,timing_correction,decision_ratio",
    )?;
    let mut frames = csv(
        &args.frames_csv,
        "candidate,start_sample,start_seconds,preamble_matches,sync_matches,quality,tone_concentration,carrier_1200,carrier_2200,decision_ratio,profile_0,profile_1,profile_2,profile_3,clock_estimated,initial_period,final_period,header_crc_valid,raw_kind,raw_transfer_id,raw_sequence,raw_payload_bytes,decoded_bits,reference_frame,reference_bits,compared_bits,bit_errors,missing_bits,ber,low_confidence,crc_result,mode,equalizer_memory,equalizer_validation,equalizer_period,rejected_calibrated_sync,trained_acquisition_ratio,equalizer_timing_offset_samples,acquisition_window_samples",
    )?;
    let recording = read_wav(&args.recording)?;
    let mut attempts = scan(&recording, options)?;
    if args.candidate.is_some_and(|i| i >= attempts.len()) {
        bail!("candidate index out of range");
    }
    if args.probe_energy {
        let attempt = &mut attempts[args.candidate.unwrap()];
        println!(
            "DIAGNOSTIC ENERGY PROBE: normal result={:?}; normal decoded bits={}; explicit reference length only; not a reception result",
            attempt.result,
            attempt.symbols.len()
        );
        (attempt.symbols, attempt.final_samples_per_symbol) = probe_energy_symbols(
            &recording,
            attempt.acquisition.clone(),
            options,
            references[args.reference_frame.unwrap()].bits.len(),
        )?;
        attempt.equalizer = None;
        // The probe is not a receive operation, even if its CRC happens to pass.
        let probe_bytes = bits_to_bytes(&attempt.bits()[..attempt.symbols.len() / 8 * 8])
            .context("byte-aligned probe")?;
        attempt.result =
            tonequill_core::protocol::framing::decode_frame(&probe_bytes).map_err(Into::into);
    }
    let mut matched = vec![false; references.len()];
    let mut valid = 0;
    let mut all_match = true;
    for (index, attempt) in attempts.iter().enumerate() {
        if args.candidate.is_some_and(|i| i != index) {
            continue;
        }
        let bits = attempt.bits();
        let bytes =
            bits_to_bytes(&bits[..bits.len() / 8 * 8]).context("unaligned diagnostic bytes")?;
        let reference = args
            .reference_frame
            .or_else(|| reference_index(&bytes, &references));
        let expected = reference.map(|i| references[i].bits.as_slice());
        let comparison = expected.map(|expected| compare_bits(expected, &bits));
        let acq = &attempt.acquisition;
        println!(
            "equalizer={:?}; rejected calibrated acquisition={:?}; trained acquisition ratio={:?}; acquisition window={}",
            attempt.equalizer,
            acq.rejected_calibrated_score,
            acq.trained_acquisition_ratio,
            acq.acquisition_window_samples
        );
        let low = attempt
            .symbols
            .iter()
            .filter(|s| s.confidence < 0.5)
            .count();
        let crc = crc_description(attempt, &bytes);
        let header = raw_header(&bytes);
        println!(
            "\nRX candidate {index}: start={:.2} samples / {:.6} s; preamble={}/64; sync={}/16; quality={:.6}; concentration={:.6}",
            acq.start_sample,
            acq.start_sample / f64::from(SAMPLE_RATE),
            acq.preamble_matches,
            acq.sync_matches,
            acq.quality,
            acq.tone_concentration
        );
        println!(
            "carrier powers: {:.9e}/{:.9e}; threshold={:.8}; profile={:.8?}; training errors={}/{}",
            acq.carrier_power[0],
            acq.carrier_power[1],
            acq.decision_ratio,
            acq.preamble_decision_ratios,
            acq.power_ratio_training_errors,
            acq.decision_training_errors
        );
        println!(
            "clock estimated={}; period initial/final={:.9}/{:.9}; ppm={:.3}",
            acq.clock_estimated,
            acq.samples_per_symbol,
            attempt.final_samples_per_symbol,
            (acq.samples_per_symbol / 480.0 - 1.0) * 1e6
        );
        println!(
            "header CRC-valid={}; raw kind/id/seq/payload={header:?}; decoded_bits={}; low_confidence={low}; {crc}",
            attempt.result.is_ok(),
            bits.len()
        );
        if let Some(i) = reference {
            let c = comparison.as_ref().unwrap();
            println!(
                "reference={i} (diagnostic comparison: {}; CRC validity is separate); compared={}; errors={}; missing={}; BER={:.9}",
                if args.reference_frame.is_some() {
                    "explicit user selection"
                } else {
                    "unique exact raw header match"
                },
                c.compared_bits,
                c.bit_errors,
                c.missing_bits,
                c.ber().unwrap_or(f64::NAN)
            );
            println!(
                "error positions: {:?}",
                bits.iter()
                    .zip(&references[i].bits)
                    .enumerate()
                    .filter_map(|(i, (a, b))| (a != b).then_some(i))
                    .collect::<Vec<_>>()
            );
            if attempt
                .result
                .as_ref()
                .is_ok_and(|p| *p == references[i].packet)
            {
                matched[i] = true;
            } else {
                all_match = false;
            }
        } else {
            all_match = false;
            println!(
                "reference identity unresolved: header does not uniquely match; no BER assigned"
            );
            for (i, reference) in references.iter().enumerate() {
                let c = compare_bits(&reference.bits, &bits);
                println!(
                    "diagnostic alternative {i}: compared={}; differences={}; missing={}",
                    c.compared_bits, c.bit_errors, c.missing_bits
                );
            }
        }
        let mut interesting: Vec<_> = attempt
            .symbols
            .iter()
            .filter(|s| {
                s.confidence < 0.5
                    || expected
                        .and_then(|e| e.get(s.index))
                        .is_some_and(|&b| b != s.bit)
            })
            .collect();
        interesting.sort_by(|a, b| a.confidence.total_cmp(&b.confidence));
        for s in interesting.iter().take(args.limit) {
            println!(
                "bit {}: TX={:?} RX={} confidence={:.5}; P0={:.8e}; P1={:.8e}; center={:.3}; threshold={:.5}",
                s.index,
                expected.and_then(|e| e.get(s.index)).map(|b| u8::from(*b)),
                u8::from(s.bit),
                s.confidence,
                s.carrier_power[0],
                s.carrier_power[1],
                s.center_sample,
                s.decision_ratio
            );
        }
        if let Some(writer) = &mut symbols {
            for s in &attempt.symbols {
                writeln!(
                    writer,
                    "{index},{},{},{:.6},{},{},{:.9e},{:.9e},{:.8},{:.8},{:.6},{:.8}",
                    reference.map_or(String::new(), |i| i.to_string()),
                    s.index,
                    s.center_sample,
                    expected
                        .and_then(|e| e.get(s.index))
                        .map_or(String::new(), |b| u8::from(*b).to_string()),
                    u8::from(s.bit),
                    s.carrier_power[0],
                    s.carrier_power[1],
                    s.soft_value,
                    s.confidence,
                    s.timing_correction,
                    s.decision_ratio
                )?;
            }
        }
        if let Some(writer) = &mut frames {
            let c = comparison.as_ref();
            writeln!(
                writer,
                "{index},{:.6},{:.9},{},{},{:.9},{:.9},{:.9e},{:.9e},{:.9},{:.9},{:.9},{:.9},{:.9},{},{:.9},{:.9},{},{},{},{},{},{},{},{},{},{},{},{},{},\"{}\",{},{},{},{},{},{},{},{}",
                acq.start_sample,
                acq.start_sample / f64::from(SAMPLE_RATE),
                acq.preamble_matches,
                acq.sync_matches,
                acq.quality,
                acq.tone_concentration,
                acq.carrier_power[0],
                acq.carrier_power[1],
                acq.decision_ratio,
                acq.preamble_decision_ratios[0],
                acq.preamble_decision_ratios[1],
                acq.preamble_decision_ratios[2],
                acq.preamble_decision_ratios[3],
                acq.clock_estimated,
                acq.samples_per_symbol,
                attempt.final_samples_per_symbol,
                attempt.result.is_ok(),
                header.0,
                header.1,
                header.2,
                header.3,
                bits.len(),
                reference.map_or(String::new(), |i| i.to_string()),
                c.map_or(String::new(), |c| c.reference_bits.to_string()),
                c.map_or(String::new(), |c| c.compared_bits.to_string()),
                c.map_or(String::new(), |c| c.bit_errors.to_string()),
                c.map_or(String::new(), |c| c.missing_bits.to_string()),
                c.and_then(|c| c.ber())
                    .map_or(String::new(), |b| format!("{b:.9}")),
                low,
                crc.replace('"', "\"\""),
                if args.probe_energy {
                    "energy_probe"
                } else {
                    "receive"
                },
                attempt
                    .equalizer
                    .as_ref()
                    .map_or(String::new(), |m| m.memory_symbols.to_string()),
                attempt
                    .equalizer
                    .as_ref()
                    .map_or(String::new(), |m| format!("{:.9}", m.validation_error)),
                attempt
                    .equalizer
                    .as_ref()
                    .map_or(String::new(), |m| format!("{:.9}", m.samples_per_symbol)),
                acq.rejected_calibrated_score
                    .map_or(String::new(), |m| m.1.to_string()),
                acq.trained_acquisition_ratio
                    .map_or(String::new(), |r| format!("{r:.9}")),
                attempt
                    .equalizer
                    .as_ref()
                    .map_or(String::new(), |m| format!("{:.6}", m.timing_offset_samples)),
                acq.acquisition_window_samples
            )?;
        }
        valid += usize::from(attempt.result.is_ok());
    }
    if let Some(writer) = &mut symbols {
        writer.flush()?;
    }
    if let Some(writer) = &mut frames {
        writer.flush()?;
    }
    println!(
        "\ncandidates={}; CRC-valid={valid}; reference frames recovered={}/{}",
        attempts.len(),
        matched.iter().filter(|b| **b).count(),
        references.len()
    );
    if args.probe_energy {
        bail!("diagnostic energy probe completed; no reception success is claimed");
    }
    if !all_match || !matched.iter().all(|m| *m) {
        bail!(
            "recording does not recover every reference frame cleanly; diagnostics retained (use decode for whole-file integrity)"
        );
    }
    Ok(())
}

fn raw_header(bytes: &[u8]) -> (String, String, String, String) {
    if bytes.len() < PREFIX_SIZE {
        return Default::default();
    }
    (
        format!("{:02X}", bytes[11]),
        format!(
            "{:08X}",
            u32::from_be_bytes(bytes[12..16].try_into().unwrap())
        ),
        u16::from_be_bytes(bytes[16..18].try_into().unwrap()).to_string(),
        u16::from_be_bytes(bytes[18..20].try_into().unwrap()).to_string(),
    )
}
fn crc_description(attempt: &FrameAttempt, bytes: &[u8]) -> String {
    match &attempt.result {
        Ok(_) => {
            let crc = u32::from_be_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
            format!("CRC PASS expected={crc:08X} calculated={crc:08X}")
        }
        Err(error) => format!("FAILED {error}"),
    }
}

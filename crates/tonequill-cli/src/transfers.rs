use crate::wav;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::Path,
};
use tonequill_core::{
    config::{MAX_PAYLOAD_SIZE, SAMPLE_RATE},
    protocol::packet::{FLAG_REQUEST, FLAG_STATUS, Packet},
    transfer::{
        Fragmenter, MAX_FILE_SIZE, Reassembler, TransferError,
        waveform::{TransmissionPlan, frame_samples},
    },
};

pub fn parse_transfer_id(value: &str) -> Result<u32, String> {
    u32::from_str_radix(value.trim_start_matches("0x").trim_start_matches("0X"), 16)
        .map_err(|_| "expected a hexadecimal 32-bit transfer ID".to_owned())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn encode_file(input: &Path, output: &Path, legacy: bool, id: Option<u32>) -> Result<()> {
    let file = File::open(input).with_context(|| format!("cannot read {}", input.display()))?;
    let limit = if legacy {
        MAX_PAYLOAD_SIZE as u64
    } else {
        MAX_FILE_SIZE
    };
    if file.metadata()?.len() > limit {
        bail!("input exceeds limit of {limit} bytes");
    }
    let mut payload = Vec::new();
    file.take(limit + 1).read_to_end(&mut payload)?;
    if payload.len() as u64 > limit {
        bail!("input exceeds limit of {limit} bytes");
    }
    let hash: [u8; 32] = Sha256::digest(&payload).into();
    let transfer_id = id.unwrap_or_else(|| u32::from_be_bytes(hash[..4].try_into().unwrap()));
    if legacy {
        let samples = frame_samples(&Packet::new(transfer_id, 0, payload.clone()))?;
        wav::write_wav(output, &samples)?;
        println!(
            "Encoded legacy payload: {} bytes; total frames: 1; duration: {:.2} s -> {}",
            payload.len(),
            samples.len() as f64 / SAMPLE_RATE as f64,
            output.display()
        );
    } else {
        let filename = input
            .file_name()
            .context("input needs a filename")?
            .to_string_lossy();
        let fragments = Fragmenter::new(transfer_id, &filename, &payload)?;
        let plan = TransmissionPlan::new(fragments.metadata())?;
        // RIFF/WAVE has a 32-bit chunk size. Fail before creating/truncating output.
        if plan.total_samples * 2 + 36 > u32::MAX as u64 {
            bail!(
                "transmission exceeds the 4 GiB RIFF/WAV limit; split the input into smaller files"
            );
        }
        wav::write_transfer(output, fragments)?;
        println!(
            "Encoded file: {} bytes; data packets: {}; metadata frames: {}; total frames: {}; duration: {:.2} s -> {}",
            payload.len(),
            plan.data_packets,
            plan.control_frames,
            plan.total_frames,
            plan.duration_seconds(),
            output.display()
        );
        println!(
            "frame bytes: {}; modulated bits: {}",
            plan.frame_bytes, plan.modulated_bits
        );
    }
    println!("transfer id: {transfer_id:08X}; sha256: {}", hex(&hash));
    Ok(())
}

pub fn decode_file(input: &Path, output: &Path, selected_id: Option<u32>) -> Result<()> {
    let mut reassembler = Reassembler::default();
    let mut legacy: BTreeMap<u32, (Packet, usize, bool)> = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let (mut found, mut valid) = (0, 0);
    let mut first_failure = None;
    wav::visit_frames(input, |attempt| {
        found += 1;
        let packet = match attempt.result {
            Ok(packet) => {
                valid += 1;
                packet
            }
            Err(error) => {
                first_failure.get_or_insert_with(|| error.to_string());
                return Ok(());
            }
        };
        // Acoustic live captures may contain reverse-link status/control frames.
        if matches!(packet.flags, FLAG_REQUEST | FLAG_STATUS) {
            return Ok(());
        }
        if selected_id.is_some_and(|id| id != packet.transfer_id) {
            return Ok(());
        }
        ids.insert(packet.transfer_id);
        if ids.len() > 16 {
            bail!("too many transfer IDs; select one with --transfer-id HEX");
        }
        if packet.flags == 0 {
            if let Some((previous, duplicates, conflict)) = legacy.get_mut(&packet.transfer_id) {
                if previous == &packet {
                    *duplicates += 1;
                } else {
                    *conflict = true;
                }
            } else {
                legacy.insert(packet.transfer_id, (packet, 0, false));
            }
        } else if let Err(error) = reassembler.push(packet) {
            // Protocol conflicts poison the affected ID and are reported by finish.
            // Resource exhaustion cannot be bypassed by dropping packets silently.
            if matches!(error, TransferError::ResourceLimit(_)) {
                return Err(error.into());
            }
        }
        Ok(())
    })?;
    println!(
        "frames found: {found}; valid frames (CRC32): {valid}; rejected candidates: {}",
        found - valid
    );
    if ids.is_empty() {
        bail!(
            "no CRC-valid packet for requested transfer ({} candidates): {}",
            found,
            first_failure
                .as_deref()
                .unwrap_or("no matching Tonequill frame acquired")
        );
    }
    if ids.len() != 1 {
        let choices: Vec<_> = ids.iter().map(|id| format!("{id:08X}")).collect();
        bail!(
            "multiple transfers found: {}; choose --transfer-id HEX",
            choices.join(", ")
        );
    }
    let transfer_id = *ids.first().unwrap();
    if let Some((packet, duplicates, conflict)) = legacy.get(&transfer_id) {
        if *conflict || reassembler.transfer_ids().any(|id| id == transfer_id) {
            bail!("ambiguous legacy/transfer packets sharing ID {transfer_id:08X}");
        }
        fs::write(output, &packet.payload)
            .with_context(|| format!("cannot write {}", output.display()))?;
        println!(
            "Legacy single-frame success; transfer id: {transfer_id:08X}; duplicate frames: {duplicates}; final size: {} bytes -> {}",
            packet.payload.len(),
            output.display()
        );
        println!(
            "CRC32 OK; local sha256: {} (legacy has no transmitted file hash)",
            hex(&Sha256::digest(&packet.payload))
        );
        return Ok(());
    }
    let progress = reassembler.progress(transfer_id)?;
    println!(
        "transfer id: {transfer_id:08X}; duplicate frames: {}; packets recovered: {}; packets expected: {}",
        progress.duplicate_frames,
        progress.received_packets,
        progress
            .expected_packets
            .map_or("unknown (metadata missing)".to_owned(), |n| n.to_string())
    );
    let file = reassembler
        .finish(transfer_id)
        .context("transfer FAILED; output untouched")?;
    // Received filenames never participate in path construction. The only payload
    // write occurs after the whole recording, all conflicts, size and SHA checks.
    fs::write(output, &file.bytes).with_context(|| format!("cannot write {}", output.display()))?;
    println!(
        "Transfer success; final size: {} bytes; SHA-256 OK -> {}",
        file.bytes.len(),
        output.display()
    );
    println!("sha256: {}", hex(&file.metadata.sha256));
    Ok(())
}

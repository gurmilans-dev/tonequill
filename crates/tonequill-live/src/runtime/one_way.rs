//! Live microphone reception of ordinary `encode` WAVs, without a return link.
use super::{AudioPort, Link};
use crate::events::{Event, Phase, Progress, file_info};
use anyhow::Result;
use tonequill_core::{
    protocol::packet::{FLAG_DATA, FLAG_METADATA},
    transfer::{Reassembler, ReassemblyLimits, TransferError, VerifiedFile},
};

pub fn receive<P: AudioPort>(
    link: &mut Link<P>,
    idle_timeout_ms: u64,
    mut commit: impl FnMut(&VerifiedFile) -> Result<(), String>,
) -> Result<usize> {
    let mut assembler = Reassembler::new(ReassemblyLimits {
        max_transfers: 1,
        ..Default::default()
    });
    let mut selected = None;
    let mut activity = link.now_ms();
    let mut progress = None;
    link.event(
        "session",
        "role=one_way_receiver; no acoustic feedback; waiting for metadata/data",
    )?;
    link.notify(Event::StateChanged {
        phase: Phase::Listening,
    });
    loop {
        let mut changed = false;
        // Inspect the entire current batch before committing so a conflict in
        // that batch cannot be hidden by an earlier complete set of packets.
        for packet in link.packets()? {
            if !matches!(packet.flags, FLAG_METADATA | FLAG_DATA) {
                continue;
            }
            if selected.is_some_and(|id| id != packet.transfer_id) {
                continue;
            }
            let id = packet.transfer_id;
            let metadata = if packet.flags == FLAG_METADATA {
                Some(tonequill_core::transfer::Metadata::decode(
                    id,
                    &packet.payload,
                )?)
            } else {
                None
            };
            assembler.push(packet)?;
            if let Some(metadata) = metadata {
                link.notify(Event::MetadataReceived {
                    transfer_id: format!("{id:08X}"),
                    file: file_info(&metadata),
                });
            }
            selected = Some(id);
            activity = link.now_ms();
            changed = true;
        }
        if changed && let Some(id) = selected {
            let current = assembler.progress(id)?;
            link.notify(Event::StateChanged {
                phase: Phase::Receiving,
            });
            link.notify(Event::Progress {
                progress: Progress {
                    transfer_id: Some(format!("{id:08X}")),
                    received_packets: current.received_packets,
                    total_packets: current.expected_packets,
                    bytes_received: Some(current.buffered_bytes as u32),
                    duplicate_frames: current.duplicate_frames as u32,
                    ..Progress::default()
                },
            });
            let snapshot = (
                current.received_packets,
                current.expected_packets,
                current.duplicate_frames,
            );
            if progress != Some(snapshot) {
                link.message(format!(
                    "Received data: {}/{}; duplicates: {}",
                    current.received_packets,
                    current
                        .expected_packets
                        .map_or("unknown".into(), |n| n.to_string()),
                    current.duplicate_frames
                ));
                link.event("receiver_progress", &format!("{current:?}"))?;
                progress = Some(snapshot);
            }
            if current.expected_packets == Some(current.received_packets) {
                link.notify(Event::StateChanged {
                    phase: Phase::Verifying,
                });
            }
            match assembler.finish(id) {
                Ok(file) => {
                    commit(&file).map_err(anyhow::Error::msg)?;
                    let hash: String = file
                        .metadata
                        .sha256
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    link.message(format!(
                        "One-way receive success: SHA-256 verified; file saved; {} bytes; SHA-256 {hash}",
                        file.bytes.len()
                    ));
                    link.event(
                        "committed",
                        &format!(
                            "mode=one_way; sha256={hash}; bytes={}; no sender confirmation",
                            file.bytes.len()
                        ),
                    )?;
                    link.event(
                        "summary",
                        &format!(
                            "mode=one_way; verified=true; sender_confirmation=unavailable; {:?}",
                            link.statistics
                        ),
                    )?;
                    return Ok(file.bytes.len());
                }
                Err(TransferError::MissingMetadata | TransferError::MissingPackets { .. }) => (),
                Err(error) => return Err(error.into()),
            }
        }
        if link.now_ms().saturating_sub(activity) >= idle_timeout_ms {
            return Err(crate::errors::AppError::new(crate::events::ErrorCode::Timeout,
                "one-way receive timed out without a complete SHA-256-verified file; destination untouched; replay the entire encoded WAV and retry"
            ).into());
        }
    }
}

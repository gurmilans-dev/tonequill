pub mod capture;
pub mod link;
pub mod one_way;
#[cfg(test)]
pub(crate) mod tests;
use crate::audio::AudioPort;
use crate::events::{Event, Phase, Progress, file_info};
use anyhow::{Context, Result, bail};
use link::Link;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use tonequill_core::{
    protocol::packet::{FLAG_DATA, FLAG_METADATA, FLAG_REQUEST, FLAG_STATUS},
    transfer::{
        VerifiedFile,
        reliable::{ReceiveSession, ReliableError, SendSession},
    },
};
pub fn run_sender<P: AudioPort>(link: &mut Link<P>, sender: &mut SendSession<'_>) -> Result<()> {
    link.event(
        "session",
        &format!(
            "role=sender; transfer_id={:08X}; bytes={}; packets={}",
            sender.metadata().transfer_id,
            sender.metadata().file_size,
            sender.metadata().data_packets
        ),
    )?;
    let mut first = true;
    while !sender.is_complete() {
        let burst = sender.next_burst()?;
        link.transmit(&burst, first)?;
        first = false;
        sender.transmitted(link.now_ms())?;
        link.notify(Event::StateChanged {
            phase: Phase::WaitingFeedback,
        });
        'feedback: loop {
            for packet in link.packets()? {
                if packet.flags != FLAG_STATUS {
                    continue;
                }
                match sender.accept(&packet) {
                    Ok(true) => break 'feedback,
                    Ok(false) => {
                        link.event("stale_status", "ignored unrelated ID, round or window")?
                    }
                    Err(ReliableError::InvalidControl(error)) => link.invalid_control(error)?,
                    Err(error) => return Err(error.into()),
                }
            }
            if let Some(poll) = sender.tick(link.now_ms())? {
                link.notify(Event::RetransmissionRequested);
                link.message("Status timeout; requesting current receipt bitmap".to_string());
                link.event("timeout", "repoll without assuming data loss")?;
                link.transmit(&[poll], false)?;
                sender.transmitted(link.now_ms())?;
            }
        }
        link.message(format!(
            "Confirmed data: {}/{}; retransmitted data: {}; retries: {}",
            sender.statistics().acknowledged_packets,
            sender.metadata().data_packets,
            sender.statistics().retransmitted_data,
            sender.statistics().retries
        ));
        link.notify(Event::Progress {
            progress: Progress {
                transfer_id: Some(format!("{:08X}", sender.metadata().transfer_id)),
                received_packets: sender.statistics().acknowledged_packets,
                total_packets: Some(sender.metadata().data_packets),
                // The sender exposes an ACK count, not acknowledged byte ranges.
                // A partial final packet makes count * 256 an incorrect metric.
                bytes_received: sender
                    .is_complete()
                    .then_some(sender.metadata().file_size as u32),
                total_bytes: Some(sender.metadata().file_size as u32),
                retransmissions: sender.statistics().retransmitted_data,
                ..Progress::default()
            },
        });
        if sender.state() == tonequill_core::transfer::reliable::SenderState::Retransmitting {
            link.notify(Event::StateChanged {
                phase: Phase::Retransmitting,
            });
        }
        link.event(
            "sender_state",
            &format!("{:?}; {:?}", sender.state(), sender.statistics()),
        )?;
    }
    // A verified remote commit is already confirmed. Close only shortens the
    // receiver's linger; failure to send it cannot undo the received Complete.
    if let Err(error) = link.transmit(&[sender.close_packet()?], false) {
        link.message(format!(
            "Receiver commit is confirmed; Close transmission failed: {error}"
        ));
        link.notify(Event::Warning {
            error: crate::errors::failure(&error),
        });
        let _ = link.event("close_failed_after_complete", &error.to_string());
    }
    Ok(())
}

pub fn validate_receive_paths(
    output: &Path,
    events: Option<&Path>,
    capture: Option<&Path>,
) -> Result<()> {
    let mut destinations = Vec::new();
    for path in [Some(output), events, capture].into_iter().flatten() {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let destination = parent
            .canonicalize()?
            .join(path.file_name().context("expected a file destination")?);
        let key = if cfg!(windows) {
            destination.to_string_lossy().to_lowercase()
        } else {
            destination.to_string_lossy().into_owned()
        };
        if destinations.contains(&key) {
            bail!("received file, event log and microphone capture must use distinct paths");
        }
        destinations.push(key);
    }
    Ok(())
}

pub fn run_receiver<P: AudioPort>(
    link: &mut Link<P>,
    session: &mut ReceiveSession,
    idle_timeout_ms: u64,
    linger_ms: u64,
    mut commit: impl FnMut(&VerifiedFile) -> Result<(), String>,
) -> Result<()> {
    link.event("session", "role=receiver; awaiting valid metadata")?;
    link.notify(Event::StateChanged {
        phase: Phase::Listening,
    });
    let mut activity = link.now_ms();
    let mut committed_at = None;
    let mut last_progress = None;
    loop {
        for packet in link.packets()? {
            let id = packet.transfer_id;
            let relevant = session.transfer_id().is_none_or(|active| id == active)
                && matches!(packet.flags, FLAG_METADATA | FLAG_DATA | FLAG_REQUEST);
            let metadata = (relevant && packet.flags == FLAG_METADATA)
                .then(|| tonequill_core::transfer::Metadata::decode(id, &packet.payload).ok())
                .flatten();
            match session.accept(packet, &mut commit) {
                Ok(response) => {
                    if session.failure().is_none()
                        && let Some(metadata) = metadata
                    {
                        link.notify(Event::MetadataReceived {
                            transfer_id: format!("{id:08X}"),
                            file: file_info(&metadata),
                        });
                    }
                    if relevant {
                        activity = link.now_ms();
                    }
                    if session.is_committed() && committed_at.is_none() {
                        committed_at = Some(link.now_ms());
                        link.message(
                            "SHA-256 verified; file saved; sending completion status".to_string(),
                        );
                        link.event(
                            "committed",
                            "SHA-256 verified and destination save succeeded",
                        )?;
                    }
                    if let Some(reply) = response {
                        link.transmit(&[reply], false)?;
                    }
                    if let Some(error) = session.failure() {
                        return Err(crate::errors::AppError::new(
                            crate::events::ErrorCode::Integrity,
                            format!("receiver rejected transfer: {error}"),
                        )
                        .into());
                    }
                }
                Err(error) => link.invalid_control(&error.to_string())?,
            }
            if let Some(progress) = session.progress() {
                link.notify(Event::Progress {
                    progress: Progress {
                        transfer_id: session.transfer_id().map(|id| format!("{id:08X}")),
                        received_packets: progress.received_packets,
                        total_packets: progress.expected_packets,
                        bytes_received: Some(progress.buffered_bytes as u32),
                        duplicate_frames: progress.duplicate_frames as u32,
                        ..Progress::default()
                    },
                });
                if !session.is_committed() {
                    link.notify(Event::StateChanged {
                        phase: Phase::Receiving,
                    });
                }
                let snapshot = (
                    progress.received_packets,
                    progress.expected_packets,
                    progress.duplicate_frames,
                );
                if last_progress != Some(snapshot) {
                    link.message(format!(
                        "Received data: {}/{}; duplicates: {}",
                        progress.received_packets,
                        progress.expected_packets.unwrap_or(0),
                        progress.duplicate_frames
                    ));
                    link.event("receiver_progress", &format!("{progress:?}"))?;
                    last_progress = Some(snapshot);
                }
            }
        }
        if session.is_closed() {
            link.message("Transfer success; sender confirmed completion".to_string());
            return Ok(());
        }
        if committed_at.is_some_and(|start| link.now_ms().saturating_sub(start) >= linger_ms) {
            link.message("File verified and saved; sender Close was not heard before the completion-listen deadline".to_string());
            link.event(
                "linger_expired",
                "verified file retained; sender confirmation unknown",
            )?;
            return Ok(());
        }
        if committed_at.is_none() && link.now_ms().saturating_sub(activity) >= idle_timeout_ms {
            return Err(crate::errors::AppError::new(crate::events::ErrorCode::Timeout, "Receive timed out without a complete SHA-256-verified transfer; destination untouched").into());
        }
    }
}

/// Commit in the requested directory, never a received path. A failed write
/// cannot truncate an existing destination, and no Complete ACK precedes rename.
pub fn commit_file(output: &Path, file: &VerifiedFile) -> Result<()> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = parent.join(format!(
        ".tonequill-{}-{}.part",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    let mut writer = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let result = (|| {
        writer.write_all(&file.bytes)?;
        writer.sync_all()?;
        drop(writer);
        fs::rename(&temporary, output)
            .with_context(|| format!("cannot commit received file to {}", output.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

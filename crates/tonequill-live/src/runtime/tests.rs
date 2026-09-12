use super::link::EventLog;
use super::*;
use crate::audio::InputEvent;
use std::{collections::VecDeque, time::Duration};
use tonequill_core::transfer::reliable::Timing;
use tonequill_core::{
    config::SAMPLE_RATE,
    protocol::packet::{FLAG_DATA, FLAG_METADATA, Packet},
    receiver::LiveDecoder,
    transfer::{
        Fragmenter,
        reliable::{Status, StatusKind},
        waveform::frame_samples,
    },
};

pub(crate) enum Peer<'a> {
    Receiver(ReceiveSession),
    Sender(SendSession<'a>),
}
struct QueuedFrame {
    packet: Packet,
    leading_ms: u64,
    trailing_ms: u64,
}

/// Virtual speaker/microphone port. Both directions use the actual BFSK waveform
/// and incremental decoder. A peer state machine reacts to decoded samples; no
/// packet shortcut is used at the application-under-test boundary. One audio
/// frame is materialized at a time; time advances without wall-clock sleeps.
pub(crate) struct VirtualPort<'a> {
    peer: Peer<'a>,
    decoder: LiveDecoder,
    now_samples: u64,
    listening: bool,
    incoming: VecDeque<QueuedFrame>,
    samples: Vec<f32>,
    index: usize,
    chunk: usize,
    commits: usize,
    recovered: Vec<u8>,
    drop_metadata: bool,
    drop_data: Option<u16>,
    drop_complete: bool,
    gap_once: bool,
    discontinuity_interval: Option<usize>,
    next_discontinuity: usize,
    control_drops: usize,
    control_frames: usize,
    fail_write: bool,
    fail_read_after_ms: Option<u64>,
}
impl<'a> VirtualPort<'a> {
    pub(crate) fn new(peer: Peer<'a>) -> Self {
        Self {
            peer,
            decoder: LiveDecoder::default(),
            now_samples: 0,
            listening: true,
            incoming: VecDeque::new(),
            samples: Vec::new(),
            index: 0,
            chunk: 0,
            commits: 0,
            recovered: Vec::new(),
            drop_metadata: false,
            drop_data: None,
            drop_complete: false,
            gap_once: false,
            discontinuity_interval: None,
            next_discontinuity: usize::MAX,
            control_drops: 0,
            control_frames: 0,
            fail_write: false,
            fail_read_after_ms: None,
        }
    }
    pub(crate) fn queue_burst(&mut self, packets: Vec<Packet>) {
        let last = packets.len() - 1;
        for (index, packet) in packets.into_iter().enumerate() {
            self.incoming.push_back(QueuedFrame {
                packet,
                leading_ms: if index == 0 { 800 } else { 200 },
                trailing_ms: if index == last { 500 } else { 0 },
            });
        }
    }
    fn peer_samples(&mut self, samples: &[f32]) -> Result<()> {
        for attempt in self.decoder.push(samples)? {
            let Ok(packet) = attempt.result else {
                continue;
            };
            if packet.flags == FLAG_METADATA && self.drop_metadata {
                self.drop_metadata = false;
                continue;
            }
            if packet.flags == FLAG_DATA && self.drop_data == Some(packet.sequence) {
                self.drop_data = None;
                continue;
            }
            if packet.flags == FLAG_STATUS {
                self.control_frames += 1;
                if self.drop_complete
                    && matches!(Status::decode(&packet)?.kind, StatusKind::Complete(_))
                {
                    self.drop_complete = false;
                    self.control_drops += 1;
                    continue;
                }
            }
            let response = match &mut self.peer {
                Peer::Receiver(receiver) => receiver
                    .accept(packet, |file| {
                        self.commits += 1;
                        self.recovered = file.bytes.clone();
                        Ok(())
                    })?
                    .map(|p| vec![p]),
                Peer::Sender(sender) => {
                    if sender.accept(&packet)? {
                        Some(if sender.is_complete() {
                            vec![sender.close_packet()?]
                        } else {
                            sender.next_burst()?
                        })
                    } else {
                        None
                    }
                }
            };
            if let Some(packets) = response {
                self.queue_burst(packets);
            }
        }
        Ok(())
    }
}
impl AudioPort for VirtualPort<'_> {
    fn now_ms(&self) -> u64 {
        self.now_samples * 1000 / u64::from(SAMPLE_RATE)
    }
    fn set_listening(&mut self, enabled: bool) {
        self.listening = enabled;
        if !enabled {
            self.samples.clear();
            self.index = 0;
        }
    }
    fn write(&mut self, samples: &[f32]) -> Result<()> {
        assert!(!self.listening, "TX was not gated");
        if self.fail_write {
            bail!("injected output disconnect");
        }
        self.now_samples += samples.len() as u64;
        self.peer_samples(samples)
    }
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
    fn pause(&mut self, duration: Duration) -> Result<()> {
        let count = (duration.as_millis() as u64 * u64::from(SAMPLE_RATE) / 1000) as usize;
        self.now_samples += count as u64;
        // Quiet turnaround lets the peer's incremental decoder finish its poll.
        if !self.listening {
            self.peer_samples(&vec![0.0; count])?;
        }
        Ok(())
    }
    fn read(&mut self, timeout: Duration) -> Result<InputEvent> {
        if self
            .fail_read_after_ms
            .is_some_and(|limit| self.now_ms() >= limit)
        {
            bail!("injected input cancellation");
        }
        assert!(self.listening, "RX attempted during TX or settle");
        if self.gap_once && self.index > self.samples.len() / 2 && !self.samples.is_empty() {
            self.gap_once = false;
            return Ok(InputEvent::Gap);
        }
        if self.index >= self.next_discontinuity && self.index < self.samples.len() {
            self.next_discontinuity = self
                .next_discontinuity
                .saturating_add(self.discontinuity_interval.unwrap());
            return Ok(InputEvent::Discontinuity);
        }
        if self.index == self.samples.len() {
            if let Some(queued) = self.incoming.pop_front() {
                let dropped = if queued.packet.flags == FLAG_STATUS {
                    self.control_frames += 1;
                    if self.drop_complete
                        && matches!(
                            Status::decode(&queued.packet)?.kind,
                            StatusKind::Complete(_)
                        )
                    {
                        self.drop_complete = false;
                        self.control_drops += 1;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                self.samples = vec![0.0; (queued.leading_ms * 48) as usize];
                let mut waveform = frame_samples(&queued.packet)?;
                if dropped {
                    waveform.fill(0.0);
                }
                self.samples.extend(waveform);
                self.samples
                    .resize(self.samples.len() + (queued.trailing_ms * 48) as usize, 0.0);
                self.index = 0;
                self.next_discontinuity = self.discontinuity_interval.unwrap_or(usize::MAX);
                // Arm against the scheduled end, even if the tested receiver
                // starts its reply before it reads all trailing silence.
                if self.incoming.is_empty() {
                    let end = self.now_ms() + self.samples.len() as u64 / 48;
                    if let Peer::Sender(sender) = &mut self.peer
                        && sender.state().is_waiting()
                    {
                        sender.transmitted(end)?;
                    }
                }
            } else {
                let now = self.now_ms();
                if let Peer::Sender(sender) = &mut self.peer
                    && let Some(poll) = sender.tick(now)?
                {
                    self.queue_burst(vec![poll]);
                }
                self.now_samples += (timeout.as_millis() * 48) as u64;
                return Ok(InputEvent::Samples(vec![
                    0.0;
                    (timeout.as_millis() * 48) as usize
                ]));
            }
        }
        let count = [127, 480, 1024, 4096][self.chunk % 4].min(self.samples.len() - self.index);
        self.chunk += 1;
        let chunk = self.samples[self.index..self.index + count].to_vec();
        self.index += count;
        self.now_samples += count as u64;
        Ok(InputEvent::Samples(chunk))
    }
}

#[test]
fn one_way_receives_without_poll_and_never_transmits_feedback() {
    for bytes in [
        Vec::new(),
        b"phone live sample".to_vec(),
        (0..257).map(|n| n as u8).collect(),
    ] {
        let frames: Vec<_> = Fragmenter::new(0x7139a428, "phone.bin", &bytes)
            .unwrap()
            .collect();
        let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
        // DATA before metadata and duplicate DATA are supported during replay.
        let mut sequence: Vec<_> = frames.iter().skip(1).cloned().collect();
        sequence.extend(frames.clone());
        port.queue_burst(sequence);
        port.fail_write = true; // Any unexpected outgoing audio fails this test.
        let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
        let mut calls = 0;
        let length = one_way::receive(&mut link, 30000, |file| {
            calls += 1;
            assert_eq!(file.bytes, bytes);
            Ok(())
        })
        .unwrap();
        assert_eq!(length, bytes.len());
        assert_eq!(calls, 1);
        assert_eq!(link.statistics.transmitted_frames, 0);
        assert_eq!(link.statistics.control_frames, 0);
    }
}

#[test]
fn one_way_incomplete_or_hash_invalid_transfers_never_commit() {
    let frames: Vec<_> = Fragmenter::new(0x7139a428, "phone.bin", b"hello from phone")
        .unwrap()
        .collect();
    let mut bad = frames.clone();
    bad[1].payload[0] ^= 1; // Correct frame CRC, deliberately wrong end-to-end hash.
    for sequence in [
        vec![],
        vec![frames[0].clone()],
        vec![frames[1].clone()],
        bad,
    ] {
        let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
        if !sequence.is_empty() {
            port.queue_burst(sequence);
        }
        port.fail_write = true;
        let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
        let mut calls = 0;
        assert!(
            one_way::receive(&mut link, 30000, |_| {
                calls += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(calls, 0);
        assert_eq!(link.statistics.transmitted_frames, 0);
    }
}

#[test]
fn one_way_failed_commit_does_not_report_success() {
    let frames: Vec<_> = Fragmenter::new(7, "phone.bin", b"hello from phone")
        .unwrap()
        .collect();
    let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
    port.queue_burst(frames);
    let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
    let result = one_way::receive(&mut link, 30000, |_| Err("injected disk failure".into()));
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("injected disk failure")
    );
    assert_eq!(link.statistics.transmitted_frames, 0);
}

#[test]
fn cancelled_one_way_retains_the_microphone_wav_and_telemetry_without_committing_payload() {
    let path = std::env::temp_dir().join(format!(
        "tonequill-aborted-capture-{}-{}.wav",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let frames: Vec<_> = Fragmenter::new(7, "phone.bin", b"hello from phone")
        .unwrap()
        .collect();
    let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
    port.queue_burst(vec![frames[0].clone()]);
    port.fail_read_after_ms = Some(11000);
    let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
    link.input_diagnostics = Some(capture::InputDiagnostics::new(Some(&path)).unwrap());
    let error =
        one_way::receive(&mut link, 120000, |_| panic!("incomplete file committed")).unwrap_err();
    assert!(error.to_string().contains("injected input cancellation"));
    let count = link.input_diagnostics.as_ref().unwrap().samples;
    assert!(count >= 11 * 48000);
    link.finish_input_diagnostics().unwrap();
    let mut reader = hound::WavReader::open(&path).unwrap();
    assert_eq!(u64::from(reader.duration()), count);
    let samples: Vec<_> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32767.0)
        .collect();
    let decoded = tonequill_core::receiver::scan(&samples, Default::default()).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].result.as_ref().unwrap(), &frames[0]);
    drop(reader);
    fs::remove_file(path).unwrap();
}

#[test]
fn one_way_repeated_metadata_then_complete_replay_recovers_the_missing_data() {
    let expected = b"hello from phone";
    let frames: Vec<_> = Fragmenter::new(0x7b193648, "phone.bin", expected)
        .unwrap()
        .collect();
    let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
    port.queue_burst(vec![frames[0].clone(), frames[0].clone()]);
    port.queue_burst(frames);
    let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
    link.input_diagnostics = Some(capture::InputDiagnostics::new(None).unwrap());
    let mut commits = 0;
    one_way::receive(&mut link, 30000, |file| {
        commits += 1;
        assert_eq!(file.bytes, expected);
        Ok(())
    })
    .unwrap();
    assert_eq!(commits, 1);
    assert_eq!(link.statistics.received_frames, 4);
    assert_eq!(link.statistics.transmitted_frames, 0);
}

#[test]
fn backend_discontinuity_flags_do_not_discard_delivered_crc_valid_pcm() {
    let expected = b"hello from phone";
    let frames: Vec<_> = Fragmenter::new(0x7b193648, "phone.bin", expected)
        .unwrap()
        .collect();
    let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
    // WASAPI can report DATA_DISCONTINUITY before still delivering a buffer.
    // Exercise many such notifications across both multi-second frames.
    port.discontinuity_interval = Some(12_000);
    port.queue_burst(frames);
    let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
    let mut commits = 0;
    one_way::receive(&mut link, 30_000, |file| {
        commits += 1;
        assert_eq!(file.bytes, expected);
        Ok(())
    })
    .unwrap();
    assert_eq!(commits, 1);
    assert!(link.statistics.capture_gaps >= 20);
    assert_eq!(link.statistics.received_frames, 2);
}

#[test]
#[ignore = "requires local captures/phone-pc-01.wav and phone-pc-multi-02.wav"]
fn one_way_replays_physical_phone_captures_through_live_receiver() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let expected = fs::read(root.join("test-vectors/physical-hello-v0.txt")).unwrap();
    for name in ["phone-pc-01.wav", "phone-pc-multi-02.wav"] {
        let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
        port.samples = hound::WavReader::open(root.join("captures").join(name))
            .unwrap()
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32767.0)
            .collect();
        port.fail_write = true;
        let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
        let mut calls = 0;
        one_way::receive(&mut link, 20000, |file| {
            calls += 1;
            assert_eq!(file.bytes, expected);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(link.statistics.transmitted_frames, 0);
    }
}

#[test]
#[ignore = "requires local target/live-phone/mic-20260910-000232.wav"]
fn one_way_replays_first_live_microphone_capture_with_verified_commit() {
    check_live_microphone_capture("000232");
}

#[test]
#[ignore = "requires local target/live-phone captures 000500, 000529, 073454 from 2026-09-10"]
fn one_way_replays_morning_failure_and_both_successful_live_controls() {
    for trial in ["000500", "000529", "073454"] {
        check_live_microphone_capture(trial);
    }
}

#[test]
#[ignore = "requires local target/live-phone/mic-20260910-201908.wav"]
fn one_way_replays_evening_live_failure_that_decodes_offline() {
    check_live_microphone_capture("201908");
}

#[test]
#[ignore = "requires local target/live-phone successful evening captures from 2026-09-10"]
fn one_way_replays_successful_evening_live_controls() {
    for trial in [
        "201704", "201850", "201931", "201951", "202006", "210335", "210447",
    ] {
        check_live_microphone_capture(trial);
    }
}

#[test]
#[ignore = "requires local target/live-phone captures 202032 and 210359 from 2026-09-10"]
fn one_way_replays_half_window_acquisition_failures_with_verified_commit() {
    for trial in ["202032", "210359"] {
        check_live_microphone_capture(trial);
    }
}

#[test]
#[ignore = "requires local 2026-09-11 microphone capture 100609 and physical-257.bin"]
fn one_way_replays_physical_257_byte_transfer_with_verified_commit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let expected = fs::read(root.join("test-vectors/physical-257.bin")).unwrap();
    assert_eq!(expected.len(), 257);
    check_live_microphone_recording(
        &root.join("target/live-phone/mic-257-20260911-100609.wav"),
        &expected,
    );
}

fn check_live_microphone_capture(trial: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let expected = fs::read(root.join("test-vectors/physical-hello-v0.txt")).unwrap();
    check_live_microphone_recording(
        &root.join(format!("target/live-phone/mic-20260910-{trial}.wav")),
        &expected,
    );
}

fn check_live_microphone_recording(recording: &Path, expected: &[u8]) {
    let expected_frames = 1 + expected.len().div_ceil(256);
    let samples: Vec<f32> = hound::WavReader::open(recording)
        .unwrap()
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32767.0)
        .collect();
    // Check the incremental PHY with the real 10 ms device callback size,
    // including a sub-grid leading offset that changes all buffer alignments.
    for padding in [0, 137] {
        let mut shifted = vec![0.0; padding];
        shifted.extend_from_slice(&samples);
        let mut decoder = tonequill_core::receiver::LiveDecoder::default();
        let mut frames = Vec::new();
        let mut longest = Duration::ZERO;
        let start = std::time::Instant::now();
        for chunk in shifted.chunks(480) {
            let step = std::time::Instant::now();
            frames.extend(decoder.push(chunk).unwrap());
            longest = longest.max(step.elapsed());
        }
        frames.extend(decoder.flush().unwrap());
        eprintln!(
            "microphone replay recording={}, padding={padding}: {:?} total; {:?} max push",
            recording.display(),
            start.elapsed(),
            longest
        );
        let results: Vec<_> = frames
            .iter()
            .map(|a| (&a.acquisition, &a.result, &a.equalizer))
            .collect();
        assert_eq!(
            frames.len(),
            expected_frames,
            "padding={padding}: {results:?}"
        );
        assert!(
            frames.iter().all(|a| a.result.is_ok()),
            "padding={padding}: {results:?}"
        );
    }
    let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
    port.samples = samples;
    port.fail_write = true;
    let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
    let mut commits = 0;
    one_way::receive(&mut link, 45000, |file| {
        commits += 1;
        assert_eq!(file.bytes, expected);
        Ok(())
    })
    .unwrap();
    assert_eq!(commits, 1);
    assert_eq!(link.statistics.received_frames, expected_frames as u64);
    assert_eq!(link.statistics.transmitted_frames, 0);
}

#[test]
fn live_sender_uses_real_samples_and_recovers_metadata_data_and_final_ack_loss() {
    let data: Vec<u8> = (0..257).map(|i| i as u8).collect();
    let mut sender = SendSession::new(42, "binary.bin", &data, 4, 8).unwrap();
    let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
    port.drop_metadata = true;
    port.drop_data = Some(0);
    port.drop_complete = true;
    let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
    run_sender(&mut link, &mut sender).unwrap();
    assert!(sender.is_complete());
    assert_eq!(sender.statistics().metadata_transmissions, 2);
    assert_eq!(sender.statistics().retransmitted_data, 1);
    assert_eq!(link.port.commits, 1);
    assert_eq!(link.port.recovered, data);
    assert_eq!(link.port.control_drops, 1);
    assert!(matches!(&link.port.peer, Peer::Receiver(receiver) if receiver.is_closed()));
    assert!(link.port.listening);
}

#[test]
fn live_receiver_commits_once_and_answers_repeated_final_poll_over_samples() {
    let data = b"live";
    let mut sender = SendSession::new(91, "x.bin", data, 4, 8).unwrap();
    let initial = sender.next_burst().unwrap();
    let mut port = VirtualPort::new(Peer::Sender(sender));
    port.drop_complete = true;
    port.queue_burst(initial);
    let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
    let mut receiver = ReceiveSession::default();
    let mut commits = 0;
    run_receiver(&mut link, &mut receiver, 900000, 400000, |file| {
        commits += 1;
        assert_eq!(file.bytes, data);
        Ok(())
    })
    .unwrap();
    assert!(receiver.is_closed());
    assert_eq!(commits, 1);
    assert_eq!(link.port.control_drops, 1);
    assert!(matches!(&link.port.peer, Peer::Sender(sender) if sender.is_complete()));
}

#[test]
fn link_waveform_duration_matches_protocol_timing_and_transport_errors_propagate() {
    let packets: Vec<_> = Fragmenter::new(1, "x.bin", b"x").unwrap().collect();
    let timing = Timing {
        turnaround_ms: 25,
        inter_frame_guard_ms: 75,
        ..Default::default()
    };
    let port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
    let mut link = Link::new(port, timing, EventLog::open(None).unwrap()).unwrap();
    link.transmit(&packets, false).unwrap();
    assert_eq!(
        link.now_ms(),
        timing.burst_ms(&packets).unwrap() + 3 * timing.turnaround_ms
    );
    link.port.fail_write = true;
    assert!(
        link.transmit(&packets, false)
            .unwrap_err()
            .to_string()
            .contains("disconnect")
    );
}

#[test]
fn idle_receiver_and_audio_gap_do_not_commit_a_partial_file() {
    let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
    port.queue_burst(Fragmenter::new(1, "x.bin", b"x").unwrap().collect());
    port.gap_once = true; // Break metadata acquisition before any session claim.
    let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
    let mut receiver = ReceiveSession::default();
    assert!(
        run_receiver(&mut link, &mut receiver, 20000, 400000, |_| panic!(
            "partial commit"
        ))
        .is_err()
    );
    assert_eq!(link.statistics.capture_gaps, 1);
    assert!(!receiver.is_committed());
}

#[test]
fn destination_commit_replaces_only_after_verified_file_and_cleans_failed_staging() {
    let directory = std::env::temp_dir().join(format!(
        "tonequill-commit-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&directory).unwrap();
    let output = directory.join("received.bin");
    fs::write(&output, b"old user bytes").unwrap();
    let file = VerifiedFile {
        metadata: Fragmenter::new(1, "x.bin", b"new bytes")
            .unwrap()
            .metadata()
            .clone(),
        bytes: b"new bytes".to_vec(),
    };
    commit_file(&output, &file).unwrap();
    assert_eq!(fs::read(&output).unwrap(), file.bytes);
    let blocked = directory.join("directory-is-not-a-file");
    fs::create_dir(&blocked).unwrap();
    assert!(commit_file(&blocked, &file).is_err());
    assert!(blocked.is_dir());
    assert_eq!(
        fs::read_dir(&directory).unwrap().count(),
        2,
        "staging file leaked"
    );
    fs::remove_file(&output).unwrap();
    fs::remove_dir(&blocked).unwrap();
    fs::remove_dir(&directory).unwrap();
}

#[test]
#[ignore = "full 1 KiB and 10 KiB duplex BFSK sample regression; run with --release"]
fn live_sample_transfers_one_and_ten_kib_without_wav_or_devices() {
    for size in [1024, 10240] {
        let data: Vec<u8> = (0..size).map(|i| (i * 113 + i / 19) as u8).collect();
        let mut sender = SendSession::new(991, "random.bin", &data, 8, 8).unwrap();
        let mut port = VirtualPort::new(Peer::Receiver(ReceiveSession::default()));
        port.drop_data = Some(2);
        port.drop_complete = true;
        let mut link = Link::new(port, Timing::default(), EventLog::open(None).unwrap()).unwrap();
        run_sender(&mut link, &mut sender).unwrap();
        assert!(sender.is_complete());
        assert_eq!(sender.statistics().retransmitted_data, 1);
        assert_eq!(link.port.recovered, data);
        assert_eq!(link.port.commits, 1);
    }
}

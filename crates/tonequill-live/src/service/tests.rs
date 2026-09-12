use super::*;
use std::{fs, path::PathBuf};
use tonequill_core::{
    protocol::packet::Packet,
    transfer::{Reassembler, waveform::frame_samples},
};

struct PcmPort {
    samples: Vec<f32>,
    cursor: usize,
    now: u64,
    dropped: Arc<AtomicBool>,
    writes: Arc<AtomicU64>,
    slow: bool,
}
impl PcmPort {
    fn new(samples: Vec<f32>) -> Self {
        Self {
            samples,
            cursor: 0,
            now: 0,
            dropped: Arc::new(AtomicBool::new(false)),
            writes: Arc::new(AtomicU64::new(0)),
            slow: false,
        }
    }
}
impl Drop for PcmPort {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}
impl AudioPort for PcmPort {
    fn now_ms(&self) -> u64 {
        self.now
    }
    fn set_listening(&mut self, _: bool) {}
    fn read(&mut self, timeout: Duration) -> Result<InputEvent> {
        if self.slow {
            thread::sleep(Duration::from_millis(5));
        }
        if self.cursor < self.samples.len() {
            let end = (self.cursor + 4800).min(self.samples.len());
            let samples = self.samples[self.cursor..end].to_vec();
            self.now += (end - self.cursor) as u64 / 48;
            self.cursor = end;
            Ok(InputEvent::Samples(samples))
        } else {
            self.now += timeout.as_millis() as u64;
            Ok(InputEvent::Idle)
        }
    }
    fn write(&mut self, samples: &[f32]) -> Result<()> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        self.now += samples.len() as u64 / 48;
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
    fn pause(&mut self, duration: Duration) -> Result<()> {
        self.now += duration.as_millis() as u64;
        Ok(())
    }
}
fn request(path: PathBuf) -> SessionRequest {
    SessionRequest {
        direction: Direction::Receive,
        mode: TransferMode::OneWay,
        path: path.to_string_lossy().into_owned(),
        devices: DeviceSelection::default(),
        capture_path: None,
        events_path: None,
        max_seconds: 120,
        idle_timeout_seconds: 60,
        overwrite: false,
        expected_sha256: None,
    }
}
fn pcm(packets: &[Packet]) -> Vec<f32> {
    let mut samples = vec![0.0; 9600];
    for packet in packets {
        samples.extend(frame_samples(packet).unwrap());
        samples.extend(vec![0.0; 9600]);
    }
    samples.extend(vec![0.0; 48000]);
    samples
}
fn packets(bytes: &[u8]) -> Vec<Packet> {
    Fragmenter::new(42, "example.bin", bytes).unwrap().collect()
}
fn wait(manager: &SessionManager) -> SessionUpdate {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let update = manager.poll(0);
        if update
            .snapshot
            .as_ref()
            .is_some_and(|s| !matches!(s.status, SessionStatus::Active { .. }))
        {
            return update;
        }
        assert!(Instant::now() < deadline, "session did not end");
        thread::sleep(Duration::from_millis(10));
    }
}
fn verify(bytes: &[u8]) -> VerifiedFile {
    let mut assembler = Reassembler::default();
    for packet in packets(bytes) {
        assembler.push(packet).unwrap();
    }
    assembler.finish(42).unwrap()
}
#[test]
fn receive_real_pcm_counts_data_and_commits_only_after_audio_cleanup() {
    for bytes in [
        b"Hello Tonequill\r\n".to_vec(),
        (0..257).map(|n| n as u8).collect(),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("chosen-name.bin");
        let port = PcmPort::new(pcm(&packets(&bytes)));
        let dropped = port.dropped.clone();
        let writes = port.writes.clone();
        let manager = SessionManager::default();
        manager
            .start_using(request(output.clone()), |_, _| Ok(port))
            .unwrap();
        let update = wait(&manager);
        let snapshot = update.snapshot.unwrap();
        assert!(matches!(
            snapshot.status,
            SessionStatus::Completed {
                result: Completion::Received { .. }
            }
        ));
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(writes.load(Ordering::Relaxed), 0);
        assert_eq!(
            snapshot.progress.received_packets,
            bytes.len().div_ceil(256) as u32
        );
        assert_eq!(
            snapshot.progress.valid_frames,
            bytes.len().div_ceil(256) as u32 + 1
        );
        assert!(snapshot.signal.unwrap().samples > 0.0);
        assert!(snapshot.frame.is_some());
        assert_eq!(fs::read(output).unwrap(), bytes);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn one_way_sessions_open_only_the_audio_direction_they_use() {
    let dir = tempfile::tempdir().unwrap();
    let receive_flags = Arc::new(Mutex::new(None));
    let observed = receive_flags.clone();
    let receive = SessionManager::default();
    receive
        .start_using(
            request(dir.path().join("receive.bin")),
            move |options, _| {
                *observed.lock().unwrap() = Some((options.input_enabled, options.output_enabled));
                Ok(PcmPort::new(vec![]))
            },
        )
        .unwrap();
    let snapshot = wait(&receive).snapshot.unwrap();
    assert!(matches!(snapshot.status, SessionStatus::Failed { .. }));
    assert_eq!(*receive_flags.lock().unwrap(), Some((true, false)));

    let input = dir.path().join("send.bin");
    fs::write(&input, b"send one way").unwrap();
    let mut send_request = request(input);
    send_request.direction = Direction::Send;
    let send_flags = Arc::new(Mutex::new(None));
    let observed = send_flags.clone();
    let send = SessionManager::default();
    send.start_using(send_request, move |options, _| {
        *observed.lock().unwrap() = Some((options.input_enabled, options.output_enabled));
        Ok(PcmPort::new(vec![]))
    })
    .unwrap();
    let snapshot = wait(&send).snapshot.unwrap();
    assert!(matches!(snapshot.status, SessionStatus::Completed { .. }));
    assert_eq!(*send_flags.lock().unwrap(), Some((false, true)));
}
#[test]
fn data_before_metadata_duplicates_and_unknown_totals_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("received");
    let p = packets(b"payload");
    let port = PcmPort::new(pcm(&[p[1].clone(), p[1].clone(), p[0].clone()]));
    let manager = SessionManager::default();
    manager
        .start_using(request(output.clone()), |_, _| Ok(port))
        .unwrap();
    let update = wait(&manager);
    let snapshot = update.snapshot.unwrap();
    assert!(matches!(snapshot.status, SessionStatus::Completed { .. }));
    assert_eq!(snapshot.progress.duplicate_frames, 1);
    assert!(update.events.iter().any(|e| matches!(&e.event, Event::Progress { progress } if progress.total_packets.is_none() && progress.received_packets == 1)));
    assert_eq!(fs::read(output).unwrap(), b"payload");
}
#[test]
fn missing_frames_and_sha_mismatch_never_touch_destination() {
    let p = packets(b"payload");
    let mut corrupt = p[1].clone();
    corrupt.payload[0] ^= 1;
    for (frames, code) in [
        (vec![], ErrorCode::NoFrames),
        (vec![p[1].clone()], ErrorCode::MissingMetadata),
        (vec![p[0].clone()], ErrorCode::MissingPackets),
        (vec![p[0].clone(), corrupt], ErrorCode::Integrity),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("existing");
        fs::write(&output, b"keep me").unwrap();
        let mut req = request(output.clone());
        req.overwrite = true;
        let port = PcmPort::new(pcm(&frames));
        let manager = SessionManager::default();
        manager.start_using(req, |_, _| Ok(port)).unwrap();
        let status = wait(&manager).snapshot.unwrap().status;
        assert!(matches!(status, SessionStatus::Failed { error } if error.code == code));
        assert_eq!(fs::read(output).unwrap(), b"keep me");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
#[test]
fn cancellation_and_atomic_commit_agree_with_disk() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("received");
    let file = verify(b"payload");
    let cancelled = SessionControl::default();
    cancelled.cancel();
    assert!(cancelled.received(&output, &file, false).is_err());
    assert!(!output.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    let committed = SessionControl::default();
    committed.received(&output, &file, false).unwrap();
    committed.cancel();
    assert!(matches!(
        committed.result(),
        Some(Completion::Received { .. })
    ));
    assert_eq!(fs::read(&output).unwrap(), b"payload");
    assert!(
        SessionControl::default()
            .received(&output, &verify(b"other"), false)
            .is_err()
    );
    assert_eq!(fs::read(&output).unwrap(), b"payload");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    SessionControl::default()
        .received(&output, &verify(b"replacement"), true)
        .unwrap();
    assert_eq!(fs::read(output).unwrap(), b"replacement");
}
#[test]
fn cancel_joins_worker_busy_and_stale_ids_cannot_stop_next_session() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path().join("out"));
    let manager = SessionManager::default();
    let mut port = PcmPort::new(vec![]);
    port.slow = true;
    let dropped = port.dropped.clone();
    let id = manager.start_using(req.clone(), |_, _| Ok(port)).unwrap();
    assert_eq!(
        failure(
            &manager
                .start_using(req.clone(), |_, _| Ok(PcmPort::new(vec![])))
                .unwrap_err()
        )
        .code,
        ErrorCode::Busy
    );
    manager.cancel(&id).unwrap();
    assert!(dropped.load(Ordering::Acquire));
    assert!(matches!(
        manager.poll(0).snapshot.unwrap().status,
        SessionStatus::Cancelled
    ));
    let next = manager
        .start_using(req, |_, _| {
            let mut p = PcmPort::new(vec![]);
            p.slow = true;
            Ok(p)
        })
        .unwrap();
    assert_ne!(id, next);
    assert!(manager.cancel(&id).is_err());
    manager.cancel(&next).unwrap();
    assert!(!dir.path().join("out").exists());
}
#[test]
fn invalid_paths_and_diagnostic_alias_are_rejected_before_opening_audio() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out");
    let mut req = request(output.clone());
    req.capture_path = Some(output.to_string_lossy().into_owned());
    assert!(validate_request(&req).is_err());
    req.capture_path = None;
    fs::write(&output, b"existing").unwrap();
    assert!(validate_request(&req).is_err());
    req.overwrite = true;
    assert!(validate_request(&req).is_ok());
    req.max_seconds = 0;
    assert!(validate_request(&req).is_err());
}
#[test]
fn device_failure_is_typed_and_does_not_create_files() {
    let dir = tempfile::tempdir().unwrap();
    let manager = SessionManager::default();
    manager
        .start_using::<PcmPort, _>(request(dir.path().join("out")), |_, _| {
            Err(AppError::new(ErrorCode::DeviceUnavailable, "Removed microphone").into())
        })
        .unwrap();
    assert!(
        matches!(wait(&manager).snapshot.unwrap().status, SessionStatus::Failed { error } if error.code == ErrorCode::DeviceUnavailable)
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[test]
fn one_way_send_does_not_claim_remote_verification_and_detects_changed_file() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("hello.txt");
    fs::write(&input, b"hello").unwrap();
    let mut req = request(input.clone());
    req.direction = Direction::Send;
    req.expected_sha256 = Some(inspect_file(&input).unwrap().sha256);
    let manager = SessionManager::default();
    manager
        .start_using(req.clone(), |_, _| Ok(PcmPort::new(vec![])))
        .unwrap();
    assert!(matches!(
        wait(&manager).snapshot.unwrap().status,
        SessionStatus::Completed {
            result: Completion::Sent {
                peer_verified: false,
                ..
            }
        }
    ));
    manager.shutdown();
    fs::write(&input, b"different").unwrap();
    manager
        .start_using(req, |_, _| Ok(PcmPort::new(vec![])))
        .unwrap();
    assert!(
        matches!(wait(&manager).snapshot.unwrap().status, SessionStatus::Failed { error } if error.code == ErrorCode::InvalidArgument)
    );
}
#[test]
fn optional_physical_257_recording_passes_through_desktop_service() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/live-phone/mic-257-20260911-100609.wav");
    if !path.exists() {
        eprintln!("SKIP: optional local physical WAV not present");
        return;
    }
    let mut reader = hound::WavReader::open(path).unwrap();
    let samples = reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("257.bin");
    let manager = SessionManager::default();
    manager
        .start_using(request(output.clone()), |_, _| Ok(PcmPort::new(samples)))
        .unwrap();
    assert!(matches!(
        wait(&manager).snapshot.unwrap().status,
        SessionStatus::Completed {
            result: Completion::Received { .. }
        }
    ));
    let reference =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors/physical-257.bin"))
            .unwrap();
    assert_eq!(fs::read(output).unwrap(), reference);
}

#[test]
fn crc_corruption_is_rejected_and_cannot_become_a_file() {
    use tonequill_core::{
        modulation::bfsk::modulate,
        protocol::{bitstream::bytes_to_bits, framing::encode_frame},
    };
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out");
    let mut frame = encode_frame(&packets(b"payload")[1]).unwrap();
    *frame.last_mut().unwrap() ^= 1;
    let mut samples = vec![0.0; 9600];
    samples.extend(modulate(&bytes_to_bits(&frame)));
    samples.extend(vec![0.0; 48000]);
    let manager = SessionManager::default();
    manager
        .start_using(request(output.clone()), |_, _| Ok(PcmPort::new(samples)))
        .unwrap();
    let snapshot = wait(&manager).snapshot.unwrap();
    assert!(
        matches!(snapshot.status, SessionStatus::Failed { error } if error.code == ErrorCode::CrcFailure)
    );
    assert!(snapshot.progress.crc_failures > 0);
    assert!(!output.exists());
}

#[test]
fn closed_service_rejects_queued_start_without_opening_audio() {
    let dir = tempfile::tempdir().unwrap();
    let manager = SessionManager::default();
    manager.close();
    let error = manager
        .start_using::<PcmPort, _>(request(dir.path().join("out")), |_, _| {
            panic!("Audio must not open after close")
        })
        .unwrap_err();
    assert_eq!(failure(&error).code, ErrorCode::Busy);
}

#[test]
fn reliable_send_and_receive_use_the_same_owned_service_with_real_pcm() {
    use crate::runtime::tests::{Peer, VirtualPort};
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("source.bin");
    fs::write(&input, b"payload").unwrap();
    let manager = SessionManager::default();
    let mut req = request(input);
    req.direction = Direction::Send;
    req.mode = TransferMode::Reliable;
    manager
        .start_using(req, |_, _| {
            Ok(VirtualPort::new(Peer::Receiver(ReceiveSession::default())))
        })
        .unwrap();
    assert!(matches!(
        wait(&manager).snapshot.unwrap().status,
        SessionStatus::Completed {
            result: Completion::Sent {
                peer_verified: true,
                ..
            }
        }
    ));
    manager.shutdown();
    let output = dir.path().join("received.bin");
    let mut req = request(output.clone());
    req.mode = TransferMode::Reliable;
    manager
        .start_using(req, |_, _| {
            let mut peer = SendSession::new(7, "peer.bin", b"payload", 8, 8)?;
            let first = peer.next_burst()?;
            let mut port = VirtualPort::new(Peer::Sender(peer));
            port.queue_burst(first);
            Ok(port)
        })
        .unwrap();
    assert!(matches!(
        wait(&manager).snapshot.unwrap().status,
        SessionStatus::Completed {
            result: Completion::Received { .. }
        }
    ));
    assert_eq!(fs::read(output).unwrap(), b"payload");
}

#[test]
fn diagnostic_capture_is_finalized_after_cancel_and_logs_remain_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("mic.wav");
    let mut req = request(dir.path().join("out"));
    req.capture_path = Some(wav.to_string_lossy().into_owned());
    let manager = SessionManager::default();
    let id = manager
        .start_using(req, |_, _| {
            let mut port = PcmPort::new(vec![0.0; 48000]);
            port.slow = true;
            Ok(port)
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !wav.exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    manager.cancel(&id).unwrap();
    assert!(hound::WavReader::open(wav).is_ok());
    assert!(!dir.path().join("out").exists());
    let mut locked = manager.data.lock().unwrap();
    let data = locked.as_mut().unwrap();
    data.snapshot.status = SessionStatus::Active {
        phase: Phase::Listening,
    };
    for _ in 0..1000 {
        data.publish(Event::Signal {
            metrics: SignalMetrics::default(),
        });
    }
    drop(locked);
    let update = manager.poll(0);
    assert_eq!(update.events.len(), EVENT_LIMIT);
    assert!(update.events_truncated);
    assert!(
        manager
            .poll(update.snapshot.unwrap().last_sequence)
            .events
            .is_empty()
    );
}

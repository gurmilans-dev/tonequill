use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use tonequill_core::{
    modulation::bfsk::modulate,
    protocol::{
        bitstream::bytes_to_bits,
        framing::{PREFIX_SIZE, encode_frame},
        packet::Packet,
    },
};

struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "tonequill-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        // This unique directory is created exclusively by this test process.
        let root = self.0.canonicalize().unwrap();
        assert!(root.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        fs::remove_dir_all(root).unwrap();
    }
}

fn wav(path: &Path, samples: &[f32]) {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for &sample in samples {
        writer.write_sample((sample * 32767.0) as i16).unwrap();
    }
    writer.finalize().unwrap();
}
fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tonequill"))
}

#[test]
fn receive_diagnostic_destinations_cannot_alias_the_payload_or_each_other() {
    let directory = TestDirectory::new();
    let output = directory.path("received.bin");
    for extra in ["--capture-wav", "--events"] {
        let result = cli()
            .arg("receive")
            .arg(&output)
            .arg("--one-way")
            .arg(extra)
            .arg(&output)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("distinct paths"));
        assert!(!output.exists());
    }
    let diagnostics = directory.path("shared-diagnostics");
    let result = cli()
        .arg("receive")
        .arg(&output)
        .arg("--one-way")
        .arg("--events")
        .arg(&diagnostics)
        .arg("--capture-wav")
        .arg(&diagnostics)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!diagnostics.exists());
    assert!(!output.exists());
}

#[test]
fn per_frame_diagnostics_keep_corrupt_identity_untrusted_and_probe_is_measurement_only() {
    use tonequill_core::transfer::{Fragmenter, waveform::frame_samples};
    let directory = TestDirectory::new();
    let reference = directory.path("reference.wav");
    let recording = directory.path("recording.wav");
    let csv = directory.path("frames.csv");
    let mut clean = vec![0.0; 9600];
    let mut corrupt = clean.clone();
    let packets: Vec<_> = Fragmenter::new(0x719285ac, "hello.txt", b"arbitrary bytes!!")
        .unwrap()
        .collect();
    let mut metadata_bits = 0;
    for (i, packet) in packets.iter().enumerate() {
        clean.extend(frame_samples(packet).unwrap());
        clean.extend([0.0; 9600]);
        let mut bits = bytes_to_bits(&encode_frame(packet).unwrap());
        if i == 0 {
            metadata_bits = bits.len();
            bits[40] = !bits[40];
            bits[96] = !bits[96];
        }
        corrupt.extend(modulate(&bits));
        corrupt.extend([0.0; 9600]);
    }
    wav(&reference, &clean);
    wav(&recording, &corrupt);
    let result = cli()
        .arg("diagnose-frames")
        .arg(&reference)
        .arg(&recording)
        .arg("--frames-csv")
        .arg(&csv)
        .args(["--limit", "0"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("reference identity unresolved"), "{text}");
    assert!(text.contains("reference frames recovered=1/2"), "{text}");
    let csv_text = fs::read_to_string(&csv).unwrap();
    let rows: Vec<_> = csv_text
        .lines()
        .map(|l| l.split(',').collect::<Vec<_>>())
        .collect();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.len() == 39));
    assert_eq!(rows[0][36], "trained_acquisition_ratio");
    assert_eq!(rows[0][37], "equalizer_timing_offset_samples");
    assert_eq!(rows[0][38], "acquisition_window_samples");
    assert_eq!(rows[1][17], "false");
    assert_eq!(rows[1][23], "");
    assert_eq!(rows[2][17], "true");
    let probe = cli()
        .arg("diagnose-frames")
        .arg(&reference)
        .arg(&recording)
        .args([
            "--candidate",
            "0",
            "--reference-frame",
            "0",
            "--probe-energy",
            "--no-equalizer",
            "--limit",
            "0",
        ])
        .arg("--frames-csv")
        .arg(&csv)
        .output()
        .unwrap();
    assert!(!probe.status.success());
    let text = String::from_utf8(probe.stdout).unwrap();
    assert!(text.contains("DIAGNOSTIC ENERGY PROBE"));
    assert!(
        text.contains(&format!("compared={metadata_bits}; errors=2; missing=0")),
        "{text}"
    );
    assert!(fs::read_to_string(csv).unwrap().contains(",energy_probe,"));
    let output = directory.path("received.txt");
    for existing in [false, true] {
        if existing {
            fs::write(&output, b"preserve this file").unwrap();
        }
        let result = cli()
            .arg("decode")
            .arg(&recording)
            .arg(&output)
            .output()
            .unwrap();
        assert!(!result.status.success());
        if existing {
            assert_eq!(fs::read(&output).unwrap(), b"preserve this file");
        } else {
            assert!(!output.exists());
        }
    }
}

#[test]
#[ignore = "requires all three local captures/phone-pc*.wav and target/physical references; run in release mode"]
fn local_phone_speaker_recordings_recover_all_frames_bits_and_file_bytes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let directory = TestDirectory::new();
    for (name, reference, frames) in [
        ("phone-pc-01", "tx-hello.wav", 2),
        ("phone-pc-multi-02", "tx-hello.wav", 2),
        ("phone-pc-legacy-01", "phy-reference.wav", 1),
    ] {
        let capture = root.join(format!("captures/{name}.wav"));
        let reference = root.join("target/physical").join(reference);
        assert!(
            capture.is_file() && reference.is_file(),
            "physical evidence is missing"
        );
        let read_samples = |path: &Path| {
            hound::WavReader::open(path)
                .unwrap()
                .samples::<i16>()
                .map(|v| v.unwrap() as f32 / 32767.0)
                .collect::<Vec<_>>()
        };
        let expected =
            tonequill_core::receiver::scan(&read_samples(&reference), Default::default()).unwrap();
        let samples = read_samples(&capture);
        let mut live = tonequill_core::receiver::LiveDecoder::default();
        let mut received = Vec::new();
        for chunk in samples.chunks(12000) {
            received.extend(live.push(chunk).unwrap());
        }
        received.extend(live.flush().unwrap());
        let valid: Vec<_> = received.iter().filter(|a| a.result.is_ok()).collect();
        assert_eq!(
            valid.len(),
            expected.len(),
            "incremental physical replay: {name}; results={:?}",
            received
                .iter()
                .map(|a| (a.acquisition.start_sample, &a.result))
                .collect::<Vec<_>>()
        );
        for (a, b) in valid.iter().zip(&expected) {
            assert_eq!(a.result, b.result);
            assert_eq!(a.bits(), b.bits());
        }
        let result = cli()
            .arg("diagnose-frames")
            .arg(reference)
            .arg(&capture)
            .args(["--limit", "0"])
            .output()
            .unwrap();
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(
            result.status.success(),
            "{name}: {text}; {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            text.matches("errors=0; missing=0; BER=0.000000000").count(),
            frames
        );
        assert!(
            text.contains(&format!("reference frames recovered={frames}/{frames}")),
            "{text}"
        );
        let output = directory.path(&format!("{name}.txt"));
        let result = cli()
            .arg("decode")
            .arg(capture)
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            fs::read(output).unwrap(),
            fs::read(root.join("test-vectors/physical-hello-v0.txt")).unwrap()
        );
    }
}

#[test]
fn protocol_evaluation_emits_parseable_csv_and_rejects_invalid_campaigns() {
    let result = cli()
        .args([
            "evaluate-transfer",
            "--include-one-way",
            "--file-sizes",
            "1024",
            "--windows",
            "1,8",
            "--data-loss",
            "0",
            "--feedback-loss",
            "0",
            "--trials",
            "2",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let csv = String::from_utf8(result.stdout).unwrap();
    let rows: Vec<Vec<_>> = csv.lines().map(|line| line.split(',').collect()).collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].len(), 43);
    for row in &rows[1..] {
        assert_eq!(row.len(), rows[0].len());
        assert_eq!(row[8], "2"); // All seeded zero-loss sessions complete.
        assert_eq!(row[17], "0"); // No data retries.
        assert_eq!(row[26], "2048");
        assert!(row[34].parse::<f64>().unwrap() > 0.0);
    }
    assert_eq!(rows[1][12], "0"); // One-way has no sender confirmation.
    for arguments in [
        ["--trials", "0"],
        ["--windows", "33"],
        ["--data-loss", "NaN"],
    ] {
        assert!(
            !cli()
                .arg("evaluate-transfer")
                .args(arguments)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}

#[test]
fn offline_transfer_ignores_reverse_link_controls_in_a_capture() {
    use tonequill_core::transfer::{
        Fragmenter,
        reliable::{Request, RequestKind, Status, StatusKind},
        waveform::frame_samples,
    };
    let directory = TestDirectory::new();
    let recording = directory.path("duplex-capture.wav");
    let output = directory.path("recovered.bin");
    let request = Request {
        transfer_id: 7,
        round: 1,
        base: 0,
        count: 0,
        kind: RequestKind::Poll,
    };
    let mut frames = vec![
        request.packet().unwrap(),
        Status {
            request,
            received: 0,
            kind: StatusKind::NeedMetadata,
        }
        .packet()
        .unwrap(),
    ];
    frames.extend(Fragmenter::new(7, "x.bin", b"unchanged offline bytes").unwrap());
    let mut samples = vec![0.0; 9600];
    for frame in frames {
        samples.extend(frame_samples(&frame).unwrap());
        samples.extend([0.0; 9600]);
    }
    wav(&recording, &samples);
    let result = cli()
        .arg("decode")
        .arg(recording)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(output).unwrap(), b"unchanged offline bytes");
}

#[test]
fn crc_failure_neither_creates_nor_overwrites_payload_output() {
    let directory = TestDirectory::new();
    let input = directory.path("corrupt.wav");
    let output = directory.path("payload.bin");
    let mut frame = encode_frame(&Packet::new(7, 0, b"protected payload".to_vec())).unwrap();
    frame[PREFIX_SIZE + 2] ^= 1;
    wav(&input, &modulate(&bytes_to_bits(&frame)));
    let result = cli()
        .arg("decode")
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!output.exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("CRC mismatch"));
    fs::write(&output, b"existing user data").unwrap();
    assert!(
        !cli()
            .arg("decode")
            .arg(&input)
            .arg(&output)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(&output).unwrap(), b"existing user data");
}

#[test]
fn diagnose_retains_error_metrics_and_returns_failure_status() {
    let directory = TestDirectory::new();
    let reference = directory.path("reference.wav");
    let input = directory.path("corrupt.wav");
    let csv = directory.path("symbols.csv");
    let mut frame = encode_frame(&Packet::new(7, 0, b"protected payload".to_vec())).unwrap();
    wav(&reference, &modulate(&bytes_to_bits(&frame)));
    frame[PREFIX_SIZE + 2] ^= 1;
    wav(&input, &modulate(&bytes_to_bits(&frame)));
    let result = cli()
        .arg("diagnose")
        .arg(reference)
        .arg(input)
        .arg("--symbols-csv")
        .arg(&csv)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("errors: 1"));
    assert_eq!(
        fs::read_to_string(csv).unwrap().lines().count(),
        frame.len() * 8 + 1
    );
}

#[test]
fn cli_roundtrip_preserves_binary_payload() {
    let directory = TestDirectory::new();
    let input = directory.path("input.bin");
    let audio = directory.path("signal.wav");
    let output = directory.path("output.bin");
    let payload: Vec<u8> = (0..=255).collect();
    fs::write(&input, &payload).unwrap();
    assert!(
        cli()
            .arg("encode")
            .arg(input)
            .arg(&audio)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        cli()
            .arg("decode")
            .arg(audio)
            .arg(&output)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(output).unwrap(), payload);
}

fn cli_transfer_roundtrip(size: usize) {
    let directory = TestDirectory::new();
    let input = directory.path("source.bin");
    let audio = directory.path("transfer.wav");
    let output = directory.path("chosen-destination.bin");
    let mut state = 0x593e71a9_u32;
    let data: Vec<u8> = (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    fs::write(&input, &data).unwrap();
    let tx = cli()
        .arg("encode")
        .arg(&input)
        .arg(&audio)
        .output()
        .unwrap();
    assert!(
        tx.status.success(),
        "{}",
        String::from_utf8_lossy(&tx.stderr)
    );
    let expected = format!("data packets: {}", size.div_ceil(256));
    assert!(String::from_utf8_lossy(&tx.stdout).contains(&expected));
    let fragments = tonequill_core::transfer::Fragmenter::new(1, "source.bin", &data).unwrap();
    let plan =
        tonequill_core::transfer::waveform::TransmissionPlan::new(fragments.metadata()).unwrap();
    let reader = hound::WavReader::open(&audio).unwrap();
    assert_eq!(u64::from(reader.duration()), plan.total_samples);
    assert_eq!(
        fs::metadata(&audio).unwrap().len(),
        44 + plan.total_samples * 2
    );
    drop(reader);
    let rx = cli()
        .arg("decode")
        .arg(&audio)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        rx.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&rx.stdout),
        String::from_utf8_lossy(&rx.stderr)
    );
    assert!(String::from_utf8_lossy(&rx.stdout).contains("SHA-256 OK"));
    assert_eq!(fs::read(output).unwrap(), data);
    // RX honors the explicit output name, not the source basename in metadata.
    assert_eq!(fs::read(input).unwrap(), data);
}

#[test]
fn cli_empty_one_byte_and_payload_plus_one_roundtrip() {
    for size in [0, 1, 257] {
        cli_transfer_roundtrip(size);
    }
}

#[test]
fn cli_one_kib_binary_wav_roundtrip() {
    cli_transfer_roundtrip(1024);
}

#[test]
#[ignore = "expensive full WAV transfer: 10 KiB takes about 15 minutes of audio (88 MB); run in release mode"]
fn cli_ten_kib_binary_wav_roundtrip() {
    cli_transfer_roundtrip(10 * 1024);
}

fn packet_wav(path: &Path, packets: &[Packet]) {
    let mut samples = vec![0.0; 713];
    for packet in packets {
        samples.extend(modulate(&bytes_to_bits(&encode_frame(packet).unwrap())));
        samples.extend(vec![0.0; 9600]);
    }
    wav(path, &samples);
}

#[test]
fn incomplete_hash_invalid_conflicting_and_unsafe_transfers_never_write_output() {
    use tonequill_core::transfer::Fragmenter;
    for variant in 0..6 {
        let directory = TestDirectory::new();
        let input = directory.path("bad.wav");
        let output = directory.path("output.bin");
        let mut packets: Vec<_> = Fragmenter::new(7, "safe.bin", b"binary payload")
            .unwrap()
            .collect();
        let expected_error = match variant {
            0 => {
                packets.pop();
                "missing 1 data packets"
            }
            1 => {
                packets[0].payload[17] ^= 1;
                "SHA-256 mismatch"
            }
            2 => {
                let mut conflict = packets[1].clone();
                conflict.payload[0] ^= 1;
                packets.push(conflict);
                "conflicting duplicate"
            }
            3 => {
                let mut conflict = packets[0].clone();
                conflict.payload[17] ^= 1;
                packets.push(conflict);
                "conflicting metadata"
            }
            4 => {
                let name = b"../outside.bin";
                packets[0].payload.truncate(50);
                packets[0].payload[49] = name.len() as u8;
                packets[0].payload.extend(name);
                "unsafe or overlong filename"
            }
            _ => {
                packets.remove(0);
                "metadata is missing"
            }
        };
        packet_wav(&input, &packets);
        for existing in [false, true] {
            if existing {
                fs::write(&output, b"existing user data").unwrap();
            }
            let rx = cli()
                .arg("decode")
                .arg(&input)
                .arg(&output)
                .output()
                .unwrap();
            assert!(!rx.status.success(), "variant={variant}");
            assert!(
                String::from_utf8_lossy(&rx.stderr).contains(expected_error),
                "{}",
                String::from_utf8_lossy(&rx.stderr)
            );
            if existing {
                assert_eq!(fs::read(&output).unwrap(), b"existing user data");
            } else {
                assert!(!output.exists());
            }
        }
    }
}

#[test]
fn several_transfers_require_explicit_selection_and_duplicates_are_counted() {
    use tonequill_core::transfer::Fragmenter;
    let directory = TestDirectory::new();
    let input = directory.path("multiple.wav");
    let output = directory.path("chosen.bin");
    let mut packets: Vec<_> = Fragmenter::new(0xaa, "a.bin", b"first").unwrap().collect();
    packets.extend(Fragmenter::new(0xbb, "b.bin", b"second").unwrap());
    packets.push(packets[3].clone());
    packet_wav(&input, &packets);
    let rx = cli()
        .arg("decode")
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();
    assert!(!rx.status.success());
    assert!(String::from_utf8_lossy(&rx.stderr).contains("multiple transfers"));
    assert!(!output.exists());
    let rx = cli()
        .arg("decode")
        .arg(&input)
        .arg(&output)
        .args(["--transfer-id", "BB"])
        .output()
        .unwrap();
    assert!(
        rx.status.success(),
        "{}",
        String::from_utf8_lossy(&rx.stderr)
    );
    assert!(String::from_utf8_lossy(&rx.stdout).contains("duplicate frames: 1"));
    assert_eq!(fs::read(&output).unwrap(), b"second");
}

#[test]
fn legacy_encode_decode_and_diagnose_remain_available() {
    let directory = TestDirectory::new();
    let source = directory.path("source.bin");
    let audio = directory.path("legacy.wav");
    let output = directory.path("out.bin");
    fs::write(&source, b"legacy").unwrap();
    assert!(
        cli()
            .arg("encode")
            .arg(&source)
            .arg(&audio)
            .arg("--legacy")
            .output()
            .unwrap()
            .status
            .success()
    );
    let rx = cli()
        .arg("decode")
        .arg(&audio)
        .arg(&output)
        .output()
        .unwrap();
    assert!(rx.status.success());
    assert_eq!(fs::read(output).unwrap(), b"legacy");
    assert!(String::from_utf8_lossy(&rx.stdout).contains("legacy has no transmitted file hash"));
    assert!(
        cli()
            .arg("diagnose")
            .arg(&audio)
            .arg(&audio)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        cli()
            .arg("encode")
            .arg(&source)
            .arg(&audio)
            .output()
            .unwrap()
            .status
            .success()
    );
    let diagnostic = cli()
        .arg("diagnose")
        .arg(&audio)
        .arg(&audio)
        .output()
        .unwrap();
    assert!(!diagnostic.status.success());
    assert!(String::from_utf8_lossy(&diagnostic.stderr).contains("encode --legacy"));
}

#[test]
fn encoding_limits_are_checked_before_touching_the_wav_destination() {
    let directory = TestDirectory::new();
    let input = directory.path("too-large.bin");
    let output = directory.path("existing.wav");
    fs::write(&output, b"existing user waveform").unwrap();
    // Below the core's 16 MiB sequence limit, but above RIFF's 4 GiB audio limit.
    fs::write(&input, vec![0_u8; 600_000]).unwrap();
    let tx = cli()
        .arg("encode")
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();
    assert!(!tx.status.success());
    assert!(String::from_utf8_lossy(&tx.stderr).contains("RIFF/WAV limit"));
    assert_eq!(fs::read(&output).unwrap(), b"existing user waveform");
    fs::write(&input, vec![0_u8; 257]).unwrap();
    let tx = cli()
        .arg("encode")
        .arg(&input)
        .arg(&output)
        .arg("--legacy")
        .output()
        .unwrap();
    assert!(!tx.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"existing user waveform");
}

#[test]
#[ignore = "requires the user's local transmission.wav and recording.wav at repository root"]
fn local_physical_recording_matches_reference() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = cli()
        .arg("diagnose")
        .arg(root.join("transmission.wav"))
        .arg(root.join("recording.wav"))
        .arg("--limit")
        .arg("0")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("errors: 0; missing bits: 0"));
}

#[test]
#[ignore = "requires local transmission.wav and captures/prova-20cm.wav (formerly 35cm.wav)"]
fn local_startup_recording_matches_reference() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = TestDirectory::new();
    let output = directory.path("recovered.bin");
    let recording = root.join("captures/prova-20cm.wav");
    let result = cli()
        .arg("diagnose")
        .arg(root.join("transmission.wav"))
        .arg(&recording)
        .arg("--limit")
        .arg("0")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("errors: 0; missing bits: 0"));
    assert!(
        cli()
            .arg("decode")
            .arg(recording)
            .arg(&output)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        fs::read(output).unwrap(),
        fs::read(root.join("test-vectors/physical-hello-v0.txt")).unwrap()
    );
}

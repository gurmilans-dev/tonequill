# Multi-packet file transfer

Tonequill now transfers binary files through a single WAV containing independently
decodable packets. Success requires complete data, the exact declared file size,
and the transmitted SHA-256. The PHY is unchanged. This phase uses software/WAV
tests and the two existing local physical recordings; no new physical measurements
were made, and multi-frame reliability over speaker/air/microphone remains unmeasured.

## Starting architecture and compatibility

Work began from clean commit `fdddc01` (`Add robust acquisition, timing recovery
and channel evaluation`). There were no uncommitted user changes. Existing local
WAVs and recovered files were preserved. The baseline had 29 passing normal tests
and four optional tests. No commit was made for this phase.

Previously, `encode` generated one raw packet of at most 256 bytes. `inspect`
already searched a complete in-memory recording for multiple candidates, while
`receive` and CLI `decode` returned the first CRC-valid packet. There was no
metadata, fragmentation, reassembly or transmitted file hash.

The wire layout, field widths, byte order and CRC coverage have **not changed**:

| Field | Bytes |
| --- | ---: |
| Preamble, eight `AA` bytes | 8 |
| Sync, `D391` | 2 |
| Wire version, still 1 | 1 |
| Flags / frame kind | 1 |
| Transfer ID, big-endian u32 | 4 |
| Sequence, big-endian u16 | 2 |
| Payload length, big-endian u16 | 2 |
| Payload | 0–256 |
| CRC32, big-endian | 4 |

CRC32 covers version through payload, including flags, transfer ID, sequence and
length. Bytes remain MSB-first. Per-frame overhead is 24 bytes. PHY constants
remain 48 kHz mono, 1200/2200 Hz BFSK, 480 samples/symbol, 10 ms/symbol, 100 bit/s,
amplitude 0.55. Acquisition, carrier calibration, clock estimation, timing
recovery, symbol decisions and CRC implementation were not retuned or redesigned.

Previously reserved flags now have three exact values:

| Flags | Meaning | Sequence |
| --- | --- | --- |
| `00` | Legacy raw payload | Existing raw-packet semantics |
| `01` | File metadata | Must be zero |
| `02` | File data | Zero-based fragment index |

These are mutually exclusive kinds: `03` and all other values are rejected.
Existing readers reject the new nonzero flags instead of misinterpreting file
metadata as user data. New readers accept old flag-zero recordings. `Packet::new`
still creates a legacy packet, preserving existing core callers, simulations and
PHY tests. `encode --legacy` produces the original single-frame waveform.

An incompatible outer-format version was unnecessary: the existing flag byte
represents the semantics unambiguously, and legacy payloads are never sniffed for
magic bytes. The new metadata payload has an explicit schema version of its own.

## Metadata and fragmentation

`transfer::Fragmenter` borrows an arbitrary byte slice, calculates SHA-256 and
emits metadata followed by data. Only the current packet is allocated. Its
metadata uses this exact binary representation, with all integers big-endian:

| Payload offset | Size | Value |
| ---: | ---: | --- |
| 0 | 4 | ASCII `SLTF` |
| 4 | 1 | Metadata schema version 1 |
| 5 | 8 | Original file size, u64 |
| 13 | 4 | Data packet count, u32 |
| 17 | 32 | Raw SHA-256 digest of the complete original file |
| 49 | 1 | Filename byte length, u8 |
| 50 | 1–96 | Safe ASCII basename |

Metadata carries the transfer ID in its outer CRC-protected header, without
duplicating it in the payload. Its payload occupies `50 + filename_length` bytes,
at most 146, so it always fits one frame. There is no final/control trailer.
Unknown schema versions, inconsistent sizes/counts, truncated metadata, extra
bytes and unsafe names are rejected.

The CLI defaults to the first four SHA-256 bytes as a deterministic transfer ID.
`encode --transfer-id HEX` overrides it. IDs group packets, but are not globally
unique: a collision or reused ID with different content must fail consistency or
hash checks. The same content sent under different filenames should use distinct
IDs if both are present in the same recording.

Filenames contain only ASCII letters, digits, dot, underscore and hyphen, with
at most 96 bytes. Empty names, trailing dots, path separators, colons, control
characters, `.`/`..` and Windows device names are rejected on receive. Encoding
sanitizes unsupported characters, bounds length, and uses `file.bin` when needed.
Original Unicode spelling is not preserved. Metadata names are informational:
the CLI writes **exactly the user-specified output path**, never a path composed
from received metadata.

For file size `S`, the data count is `N = ceil(S / 256)`. Data sequence `i` carries
original bytes `[256*i, min(256*(i+1), S))`, with no transfer header inside the
data payload. All non-final data packets contain 256 bytes; the last has the exact
remaining length. The metadata sequence and data sequence zero are distinguished
by flags. Empty files send one metadata frame, zero data frames, and the SHA-256
of the empty byte string.

The u16 sequence field represents indices 0–65535, allowing 65,536 data packets
and a maximum core file size of 16,777,216 bytes (16 MiB). Count is u32 so 65,536
is representable without wrapping. Larger inputs are rejected explicitly; files
are never silently truncated. Supporting larger transfers would need a future
sequence extension or higher-level transfer grouping.

## Waveform and receive scanning

Every packet uses the existing `encode_frame` and BFSK modulator, with a full
preamble and sync. The encoder inserts 200 ms of silence between frames and
200 ms at each transmission edge. The 20-symbol guard is deliberately much
longer than the 240/480-sample observation windows and allows independent
reacquisition. Its suitability for long real acoustic echoes still needs
measurement; it is not claimed to compensate arbitrary room reverberation.

`transfer::waveform::TransmissionPlan` computes exact frame bytes, bits, silence
and sample counts. The CLI writes one modulated frame at a time to the WAV writer,
instead of retaining a whole long waveform.

The new `receiver::FrameScanner` wraps the existing PHY rather than modifying it:

1. Buffer at most 49 seconds of audio, with a 24-second region of candidate starts,
   one second of left context and at least 24 seconds of lookahead.
2. Run the unchanged `inspect` on that window and retain candidates whose starts
   belong to the current region. The lookahead contains a full maximum 280-byte
   frame, including supported clock drift and timing corrections.
3. After a CRC-valid frame, suppress nested preamble-like payload candidates
   through its last validated symbol center. Report each independently acquired
   frame once, using absolute sample coordinates.
4. After failed acquisition/header/CRC candidates, continue without trusting their
   advertised length as a skip distance. A corrupted header cannot hide the next
   valid frame by claiming a long payload.
5. Advance the region by 24 seconds, preserve overlap, and flush the tail at EOF.

Duplicate acoustic repetitions remain separate valid frames; overlap rescans do
not count as duplicates. Silence and unrelated inter-frame noise do not require
manual trimming. Existing `receive` and `inspect` APIs retain their behavior;
`scan` is a convenience for in-memory callers, and `FrameScanner` supports bounded
incremental input. This is offline chunk processing, not live device audio.

The CLI reads 65,536 PCM samples per input chunk and consumes frame attempts after
each push. Audio/tone-track working storage stays bounded with recording length;
scan work grows linearly with duration with a roughly two-window overlap factor.
Reassembly stores only unique packet bytes. Defaults allow at most 16 transfer
IDs, 65,536 unique buffered data packets, and 16 MiB of buffered file data across
all IDs. Final reconstruction temporarily adds one file-size buffer. Declared
metadata sizes do not trigger eager allocation, and duplicates do not consume
another packet's storage. Resource exhaustion is an explicit failure.

## Reassembly and output behavior

`transfer::Reassembler` groups CRC-validated packets by ID using ordered maps.
Data may arrive before metadata or in arbitrary order. New metadata validates
already-buffered sequences and lengths. Metadata arriving first validates each
subsequent data packet on insertion.

Identical metadata or data repetitions increment the duplicate count. Conflicting
metadata, differing bytes for an existing sequence, invalid sequence numbers or
wrong packet lengths permanently invalidate that ID. Later packets cannot repair
an ambiguous transfer silently. Different IDs remain isolated. `progress` reports
unique data counts, expected count, duplicate count and buffered bytes;
`missing_sequences` enumerates absent data indices when metadata is available.

`finish` requires metadata, every expected sequence and exact total size, then
concatenates in sequence order and calculates SHA-256 over those bytes. A
CRC-valid altered data packet still fails this final hash. No reference WAV or
external original file is needed by the decoder.

The CLI scans **through EOF before writing**, so a conflicting duplicate after an
apparently complete transfer still prevents output. Missing packets, missing
metadata, malformed transfer semantics and hash failures neither create nor
overwrite the requested file. Successful output uses that explicit destination.
These are validation guarantees; ordinary filesystem I/O errors remain errors.

When several IDs are present, `decode` requires `--transfer-id HEX`; it does not
pick whichever transfer happened to complete first. Selecting an ID filters
reassembly storage while frame/CRC counts still describe the whole recording.
An incomplete selected transfer fails even if another transfer is complete.

Legacy recordings with one unambiguous flag-zero packet are still decoded.
Identical repetitions are counted; conflicting legacy packets or mixed
legacy/file-transfer semantics under one ID are rejected. Legacy output reports
CRC32 and a locally calculated hash, explicitly stating that no original SHA-256
was transmitted. It is never labeled a hash-verified new-format transfer.

## CLI and diagnostics

```powershell
cargo run --release -- encode input.bin transmission.wav
cargo run --release -- decode transmission.wav recovered.bin
cargo run --release -- decode recording.wav recovered.bin --transfer-id 12345678
cargo run --release -- encode input.bin transmission.wav --transfer-id 12345678
```

TX reports source size, transfer ID, data count, metadata count, total frames,
frame bytes, bits, duration and SHA-256. RX reports candidate frames found,
CRC-valid frames, rejected candidates, duplicate frames, unique data recovered,
expected data count, final byte size, SHA-256 and success. Candidate counts are
observations, not an estimate of undetected transmitted frames. A CRC-valid frame
can still contain invalid transfer semantics, which makes reassembly fail.

The existing `evaluate` remains a single-packet PHY simulation tool. `diagnose`
and `lab/physical-link.ps1` remain single-frame PHY comparison tools:

```powershell
cargo run --release -- encode test-vectors/hello.txt phy-reference.wav --legacy
cargo run --release -- diagnose phy-reference.wav recording.wav --symbols-csv symbols.csv
./lab/physical-link.ps1 -Reference phy-reference.wav -RecordingsDirectory captures
```

`diagnose` explicitly rejects a new transfer reference to avoid declaring an
entire transfer successful merely because its first metadata packet decoded.
Transfer-level verification is performed by `decode`; detailed per-packet
selection in `diagnose` is not added in this phase. Existing local legacy
references and diagnostic CSV formats continue to work.

WAV input remains 48 kHz, mono, signed 16-bit integer PCM. No resampling is added.
Standard RIFF/WAV has a 32-bit size limit; at this bitrate, a single WAV reaches
4 GiB far before the core's sequence limit. With `sample.bin` the maximum source
that fits is 506,681 bytes (about 494.81 KiB). Filename length changes that boundary.
`encode` checks the planned RIFF size before creating or truncating output. RF64,
multi-WAV output and arbitrarily large file streaming are not implemented.

## Overhead and reproducible measurements

Here **1 KB = 1 KiB = 1,024 bytes**, and 10 KB = 10,240 bytes. The examples use
`sample.bin`, a 10-byte name, so metadata occupies 60 payload bytes and its frame
occupies 84 bytes. Results depend on the safe transmitted filename length.

For filename length `L` and file size `S`:

- `N = ceil(S / 256)` data packets, one metadata frame, `F = N + 1` total frames.
- Total transmitted frame bytes: `B = S + (50 + L) + 24 * F`.
- Total modulated bits: `8 * B`.
- Modulated duration: `8 * B / 100` seconds.
- Silence duration: `0.4 + 0.2 * N` seconds.
- WAV duration is modulated duration plus silence; PCM bytes are duration × 96,000.

| File bytes | Data packets | Metadata frames | Total frames | Frame bytes | Modulated bits | Modulated time | Silence | WAV duration |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,024 | 4 | 1 | 5 | 1,204 | 9,632 | 96.32 s | 1.20 s | **97.52 s** |
| 10,240 | 40 | 1 | 41 | 11,284 | 90,272 | 902.72 s | 8.40 s | **911.12 s (15:11.12)** |

Byte overhead above file bytes is 180 bytes (17.58%) and 1,044 bytes (10.20%),
respectively. These percentages exclude unmodulated silence. PCM data sizes are
9,361,920 and 87,467,520 bytes; the actual files add a 44-byte WAV header.
Empty files still require the metadata frame: with this name, 7.12 seconds
including edge padding.

Reproduce the calculations without generating audio:

```powershell
cargo run --quiet -p tonequill-core --example transfer_overhead
```

Results are checked against actual serialized packets in tests and retained in
[`measurements/transfer-overhead.csv`](measurements/transfer-overhead.csv).
CLI tests verify actual WAV sample counts and file sizes against the plan.

## Validation

The normal suite has **54 passing tests**, with five optional tests excluded from
the default run. New coverage includes empty/one-byte/256-byte/257-byte files,
1 KiB and 10 KiB seeded binary data, fixed metadata wire bytes with the known
SHA-256 of `abc`, all transfer field mutations protected by CRC, reverse-order
reassembly, interleaved IDs, identical and conflicting duplicates, metadata
inconsistency, wrong final hashes, missing metadata/data, unsafe names, limits
and sequence 65,535 without wrapping. The 10 KiB core tests do not generate audio.

Waveform tests cover multi-frame acquisition with leading/trailing/inter-frame
silence and inter-frame noise, reordered and duplicate frames, CRC failures,
false long header lengths, nested frame bytes in a valid payload, and maximum
frames beginning just before/on/after scan boundaries at +5,000 ppm. Existing
PHY channel, alignment, startup-calibration, malformed-frame and CRC tests pass.

CLI tests perform full binary WAV roundtrips for 0, 1, 256, 257 and 1,024 bytes;
exercise transfer selection and legacy diagnostics; and prove invalid transfers
leave absent or existing output destinations untouched. RIFF and legacy limits
are checked before touching an existing waveform destination.

All five optional tests were also run successfully: the full 10 KiB WAV
encode/decode (41 frames, about 87.47 MB), both existing physical-recording
regressions, the exhaustive PHY payload/alignment sweep and extended noise-only
false-acquisition sweep. Reusing those recordings establishes compatibility,
not new evidence of multi-frame acoustic reliability. The renamed
`captures/prova-20cm.wav` is the same recording formerly called `35cm.wav`.

```powershell
cargo fmt
cargo check
cargo test
cargo clippy --workspace --all-targets -- -D warnings
cargo test --release -p tonequill-cli --test cli -- --ignored
cargo test --release -p tonequill-core --test receiver -- --ignored
```

## Files and next phase

New production modules are `tonequill-core/src/transfer/{mod,metadata,fragment,
reassembly,waveform}.rs`, `tonequill-core/src/receiver/stream.rs`, and
`tonequill-cli/src/transfers.rs`. Core `lib.rs` and `receiver.rs` export them.
`protocol/packet.rs` defines frame kinds, and `protocol/framing.rs` validates
them. CLI `main.rs` dispatches and exposes options; `wav.rs` handles bounded WAV
I/O; `diagnostics.rs` prevents partial transfer validation from appearing as
full success. Transfer semantics stay outside `main.rs` and outside DSP modules.

Tests added are `tonequill-core/tests/transfer.rs` and `multi_frame.rs`, with
extensions to core `framing_validation.rs` and CLI `tests/cli.rs`. The existing
reserved-flags test now rejects newly undefined values/combinations (`03`, `04`,
`FF`) instead of the now-defined metadata flag. `examples/transfer_overhead.rs`,
the measurement CSV, README and this report document/reproduce the change.
Core `Cargo.toml` and `Cargo.lock` add core usage of the already-present `sha2`
package; no new package versions or additional third-party packages were added.

Remaining limitations are intentional: one metadata frame is a point of failure;
any missing data prevents completion; there is no feedback, retransmission or
FEC. Long-transfer delivery probability may be substantially below single-frame
success probability. SHA-256 detects end-to-end mismatch but provides no sender
authentication. The receiver still has its documented physical-channel limits,
and 200 ms guards have not been validated across new rooms/devices. The core
sequence limit, RIFF size limit, bounded reassembly resources and ASCII filename
policy are explicit rather than hidden truncation behaviors.

The next phase should collect repeatable **new multi-frame physical recordings**
when hardware testing is possible, retain failed attempts, and report packet
and complete-file success separately. Until then, build on the software corpus
with transfer-level channel/loss experiments and useful per-packet diagnostics.
Use those measurements to design ACK/NACK and selective retransmission in a later
explicitly scoped phase, without increasing bitrate or retuning the PHY by default.

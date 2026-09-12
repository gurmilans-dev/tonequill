# Tonequill V0 receiver engineering report

Engineering pass: 7 September 2026. Tested on Windows with rustc 1.98.1.

This report records the initial engineering pass. A subsequent physical capture
exposed a startup-calibration limitation; its fix and new measurements are in
[the startup follow-up](startup-calibration.md). The original measurements below
are retained for comparison. The current suite has 29 normal and four optional
tests, including both distinct local recordings.

The receiver now recovers the existing physical recording with **0/328 bit
errors, a valid CRC, and an identical 17-byte payload**. The original receiver
made one bit error and rejected its CRC. Deterministic channel trials demonstrate
material improvements in gain tolerance, acquisition, timing and combined
impairments. These results establish software progress; one physical capture
does not establish reliability across rooms or hardware.

## 1. Initial architecture

The workspace contained two Rust crates. `tonequill-core` provided framing,
MSB-first byte/bit conversion, continuous-phase BFSK modulation, Goertzel tone
powers, aligned hard decisions and onset-based preamble synchronization.
`tonequill-cli` provided `encode`, `decode` and a recently added `diagnose` command.
Frame-length extraction, diagnostic bit comparison and repeated symbol analysis
were orchestrated in a 422-line `main.rs`.

The actual wire format was eight `AA` preamble bytes, sync `D391`, version,
flags, transfer ID, sequence, payload length, payload and CRC32. Version is 1,
flags default to zero, header fields are big-endian, and CRC covers version
through payload. Maximum payload is 256 bytes; overhead is 24 bytes. PHY values
are 48 kHz mono, 1200/2200 Hz, 480 samples/symbol and amplitude 0.55.

The initial working tree already contained changes to CLI diagnostics and
`demodulation/bfsk.rs`, plus `recovered-sync-test.txt`. Their diagnostic functionality
was preserved and developed. Original working files were also saved under
`target/engineering-baseline`. Existing payload files and WAV recordings were
not overwritten. No commits were created.

## 2. Identified limitations

The onset detector treated the first 20 ms as background noise, required an RMS
threshold of at least 0.005, and searched only within two symbols of the selected
onset. Early interference, sufficiently low gain, or noise without a clear onset
could put acquisition at the wrong location. Only 32 alternating preamble bits
were scored, allowing ambiguity between shifts of two symbols.

After acquisition, every decision used exactly 480 samples forever. Clock error
therefore accumulated across long packets. Full-symbol energy integration also
included transition contamination. There was no preamble-trained carrier
calibration and no symbol timing tracking. Diagnostics retained soft energies,
but protocol extraction and much reusable measurement work lived in the CLI.

CRC rejection already worked and was retained. Version and flag semantics were
not validated. Tests covered clean modulation, protocol integrity and one leading
silence offset, but not channel distortion, false acquisition or output-file
integrity on CLI failure.

## 3. Baseline measurements

All eight original tests passed before implementation changes. The clean local
`transmission.wav` decoded correctly. `recording.wav` acquired at sample 61890,
matched 32/32 tested preamble bits, and made one error at bit 231. Its raw carrier
powers there were 116.774 and 130.820; confidence was 0.0567. CRC expected
`945C24C0`, calculated `83273083`; no successful payload write occurred.

A deterministic channel harness was added and run against the original receiver
before replacing acquisition or demodulation. Its frozen synchronizer remains in
`crates/tonequill-core/examples/support/baseline.rs`. Section 10 gives the matched
before/after measurements.

## 4. Changes made

- Added full-recording, preamble-and-sync acquisition with normalized soft scores
  and a carrier concentration check. Early bursts do not limit the search.
- Added sliding quadrature tone tracks with f64 accumulators, 0.5 ms observation
  spacing and interpolated observation positions.
- Trained the relative carrier decision threshold from median preamble powers.
- Estimated common sample-clock offset from the phase progression of both known
  preamble carriers, with residual and agreement checks.
- Added fractional symbol positions and a gently damped timing loop driven only
  by confident data transitions. Constant-bit runs free-run from the estimated
  clock; constant zero and one maximum-length payloads are tested.
- Used central 5 ms decision windows, leaving guards around transitions. The
  10 ms acquisition window remains useful for locating symbol boundaries.
- Added core receiver reports with raw observed symbols and explicit validation
  outcomes. The receiver reads the bounded header first, then exactly the declared
  frame, and can continue searching after a damaged packet.
- Added protocol version/flag sanity checks and tests of protected-bit mutations.
- Separated CLI WAV I/O, diagnostics and evaluation from receiver logic.
- Added seeded channel simulation, CSV statistical evaluation, experimental
  ablation flags, and scripts for synthetic and repeated physical measurements.

## 5. Evidence for the significant choices

Full-recording acquisition removed the gain-floor/onset failures in the original
test set and passes an explicit early-noise-burst regression. Full preamble and
sync checks reject long alternating tones as well as noise-only inputs.

Clock estimation and fractional timing removed the approximately 44% observed BER
on maximum-length frames at ±2000 ppm. Additional tests pass constant-bit runs at
±3000 ppm. Random maximum-length frames passed 30/30 trials at both ±5000 ppm,
at each of 15 and 0 dB SNR. This is the measured range, not a claim about larger
offsets or nonlinear time warping.

Carrier calibration has a separate controlled ablation: at 0 dB SNR with a 4×
2200 Hz amplitude bias and a 137-sample, 0.3-amplitude echo, valid short packets
improved from **46/100 without calibration to 94/100 with calibration**. Both had
five acquisition failures, identifying a remaining limitation of coarse
acquisition under strong bias and noise. Counts are in
`measurements/uncalibrated-bias.csv` and `calibrated-bias.csv`.

Timing tracking has independent evidence from the real recording: switching it
off produces two errors and a CRC failure. Changing only the decision window to
10 ms produces five observed errors and one missing final bit. The default
produces zero errors with a valid CRC. The latter is a window-width ablation,
not a claim that every possible 10 ms receiver must fail.

Short decision windows sacrifice integration time. In pure AWGN at −10 dB,
the default recovered 143/200 packets; the full-symbol experimental control
recovered 200/200 with zero errors. The default favors the measured acoustic
case. No CRC-based bit flipping, reference-assisted decisions or FEC was used.

Both carriers complete integer cycles within 240 samples, so these rectangular
correlators reject DC naturally. DC-offset and direct-correlator equivalence
tests pass. No extra high-pass filter, normalization stage or filter dependency
was added without evidence. The sliding calculation avoids redoing Goertzel work
for every overlapping acquisition candidate.

## 6. Files and modules changed

| Area | Files | Responsibility |
|---|---|---|
| Front end | `core/src/signal/tones.rs`, `signal/mod.rs` | Sliding tone powers, complex phase, concentration |
| Synchronization | `core/src/demodulation/acquisition.rs`, `sync.rs`, `mod.rs` | Preamble/sync search, calibration, clock fitting; compatibility summary API |
| Demodulation | `core/src/demodulation/bfsk.rs` | Preserved aligned utilities and user-added soft metrics; clarified intended API use |
| Receiver | `core/src/receiver.rs`, `lib.rs` | Timing loop, bounded extraction, validated results, symbol/BER metrics |
| Protocol | `core/src/protocol/framing.rs`, `packet.rs`, `bitstream.rs` | Shared bounds/version constant, fail-closed feature checks, lint cleanup |
| Experiments | `core/src/simulation.rs`, `evaluation.rs`, `examples/baseline.rs`, `examples/support/baseline.rs` | Deterministic impairments, statistics and original receiver comparison |
| CLI | `cli/src/main.rs`, `wav.rs`, `diagnostics.rs`, `evaluate.rs` | Thin command orchestration, WAV I/O, concise output and CSV exports |
| Tests | `core/tests/receiver.rs`, `framing_validation.rs`, `simulation.rs`, `cli/tests/cli.rs`; tone-track unit test | Impairments, integrity, measurement contracts and physical fixture |
| Documentation/tooling | `README.md`, this report, `docs/measurements/*`, `lab/evaluate.ps1`, `lab/physical-link.ps1` | Reproducible measurements and user-run physical trials |

In this table `core` and `cli` abbreviate `crates/tonequill-core` and
`crates/tonequill-cli`. No dependency or on-air modulation change was required.

## 7. Receiver pipeline after the changes

1. Validate finite mono 48 kHz samples.
2. Build 5 ms and 10 ms quadrature tracks at both carriers.
3. Scan the complete recording for all 64 preamble bits and the 16-bit sync word.
4. Estimate relative carrier powers and common clock ratio; refine frame onset.
5. Sample soft carrier decisions at fractional symbol positions and track
   confident transitions. Confidence is an energy separation metric, not a
   calibrated probability of correctness.
6. Decode the 20-byte prefix, validate protocol fields and bound the payload size.
7. Decode the declared frame and validate its exact framing and CRC32.
8. Expose a packet only on success. Preserve failed attempts' observed symbols
   for diagnostics. CLI decode writes the first valid packet after all checks.

`find_frame_start` remains available but now uses the complete acquisition
algorithm and reports 64 preamble bits. Combining its integer summary with the
old fixed-offset demodulator discards timing/calibration; applications should use
`receiver::receive` or `receiver::inspect` instead.

## 8. Tests added and final validation

The normal suite contains **27 passing tests**, with three optional tests kept
out of routine CI. New checks cover production-modulator loopback at boundary
lengths; sample alignment, phase, gain and DC; long runs and clock offsets;
noise/echo/bias/ramp combinations; early interference; alternating preamble
without sync; silence/DC/noise/single tones; NaN/infinity; truncation; malformed
lengths; all single-bit mutations of a frame's protected region; unknown protocol
features even with a valid CRC; and recovery of a valid packet after a corrupt one.

Measurement checks verify requested SNR independently from the padded waveform,
seed reproducibility, direct/sliding tone-power agreement, and accounting of
every packet and missing bit. CLI tests verify binary roundtrip, CRC failures
leaving absent/existing output files untouched, and failed-frame CSV diagnostics.

Optional tests were also run successfully: all **257 payload lengths**, all
**480 initial sample offsets**, **200 five-second AWGN-only recordings** with no
acquisitions, and the local physical WAV regression. Runtime of the normal tests
is a few seconds on this machine; the exhaustive receiver sweep also completes
in a few seconds in release mode.

Final commands: `cargo fmt`, `cargo check`, `cargo test`, and
`cargo clippy --workspace --all-targets -- -D warnings`. The build and lint checks
are clean. The physical batch script was exercised with the real recording and
with an additional silence file: it correctly reported 1/1 and 1/2 respectively.
This tested the script's failure accounting; it did not create new air captures.

## 9. Synthetic-channel results

SNR means active received waveform power divided by **broadband sample-domain
Gaussian noise power**, measured after resampling and echo. Padding does not
change it. This is not Eb/N0 or an in-band microphone SNR. The simulator uses
SplitMix64, Box–Muller noise, linear resampling, continuous initial phase,
frequency-specific amplitude scaling and a delayed waveform copy. It deliberately
does not clip; common gain can scale the same experiment into an audio device's
range. It does not model a complete room impulse response, codecs or AGC.

Each row uses seeds starting at 1 and varying payload bytes. Except for the
original matched baseline, default leading/trailing padding is 6037/2111 samples
and initial phase is 1.3 radians. **BER counts all observed on-air frame bits**, not
just payload bits, and is conditional on successful observation. Acquisition
failures and early header rejection leave missing bits, which are counted
separately rather than treated as correct bits. PER counts every unsuccessful
reference packet, including acquisition, protocol, truncation and CRC failures.

| Channel / payload | SNR | Valid / trials | Observed bit errors / compared bits | PER |
|---|---:|---:|---:|---:|
| AWGN / 17 bytes | 15, 5, 0, −5 dB, each | 200/200 each | 0/65,600 each | 0% |
| AWGN / 17 bytes | −10 dB | 143/200 | 71/62,912 | 28.5% |
| AWGN / 17 bytes | −15 dB | 0/200 | 42/640 | 100% |
| AWGN / 17 bytes | −20 dB | 0/200 | no observed bits | 100% |
| Combined / 17 bytes | 15, 5, 0 dB, each | 100/100 each | 0/32,800 each | 0% |
| Combined / 17 bytes | −5 dB | 74/100 | 31/32,800 | 26% |
| Combined / 17 bytes | −10 dB | 0/100 | 768/27,560 | 100% |
| Combined / 256 bytes | 15, 5, 0 dB, each | 100/100 each | 0/224,000 each | 0% |
| ±5000 ppm clock / 256 bytes | 15, 0 dB, each sign and SNR | 30/30 each | 0/67,200 each | 0% |
| 8 ms echo, amplitude 0.5, 2× carrier bias / 17 bytes | 15, 5, 0 dB, each | 100/100 each | 0/32,800 each | 0% |

“Combined” means gain 0.3, a linear amplitude ramp ending at half its initial
gain, 2× gain on 2200 Hz, +1500 ppm clock stretch, a 137-sample (2.85 ms) echo of
amplitude 0.3, DC offset 0.08, and the stated noise. The original 10-trial combined
baseline did not include the ramp or DC offset.

At −10 dB AWGN the 57 packet failures split into 40 CRC, 16 protocol and one
truncation failure, with 2688 unobserved bits. At −15 dB, 196/200 trials failed
acquisition; its BER is therefore highly conditional. At −10 dB combined, failure
counts are 69 CRC, 28 protocol and three truncation failures. Every experiment
reported **zero incorrect CRC-valid packets**.

The full-symbol AWGN control additionally passed 200/200 at −10 dB, but failed
all 200 trials at −15 dB, mostly at acquisition. Raw counts and all failure
categories are retained in `measurements/*.csv`. `lab/evaluate.ps1` reproduces
these settings. The original script sweeps were rerun into a separate directory
and compared against the saved results.

Zero observed failures do not establish a zero error probability. Under
independent trials at a fixed channel setting, 0/200 failures gives an approximate
one-sided 95% upper PER bound of 1.49%; 0/100 gives 2.95%. Hardware and room
variation are not represented by these fixed synthetic conditions.

## 10. Matched before/after BER and PER

Each row below contains ten deterministic trials with the same payloads and
waveforms for both receivers. Payload is 17 bytes except for the clock rows,
which use 256 bytes. BER is conditional on compared bits in both versions.

| Case | Original errors / compared bits | Original BER | Original PER | New errors / compared bits | New PER |
|---|---:|---:|---:|---:|---:|
| Ideal | 0/3280 | 0% | 0% | 0/3280 | 0% |
| Offset 3457, trailing padding | 0/3280 | 0% | 0% | 0/3280 | 0% |
| Gain 0.005 with leading padding | 1100/3280 | 33.5366% | 100% | 0/3280 | 0% |
| AWGN 15 dB | 0/3280 | 0% | 0% | 0/3280 | 0% |
| AWGN 0 dB | 321/984 | 32.6220% | 100% | 0/3280 | 0% |
| +2000 ppm | 9821/22,400 | 43.8438% | 100% | 0/22,400 | 0% |
| −2000 ppm | 9822/22,400 | 43.8482% | 100% | 0/22,400 | 0% |
| Combined, 15 dB | 290/3280 | 8.8415% | 100% | 0/3280 | 0% |

Original 0 dB acquisition failed seven times, explaining the smaller BER
denominator. CSVs: `baseline-before.csv` and `baseline-after.csv`.

## 11. Existing real recordings

Only one air-channel recording and its clean transmission were found; both are
local ignored WAV files. The capture environment, separation, hardware and
recording processing are unknown.

| Metric | Original receiver | New default |
|---|---:|---:|
| Start sample | 61890 | 61920 |
| Tested preamble matches | 32/32 | 64/64, plus 16/16 sync |
| Acquisition quality | 0.842 | 0.87947 |
| Bit errors / compared | 1/328 | 0/328 |
| BER | 0.304878% | 0% |
| CRC / exact payload | Failure | Valid / identical |

Quality values are not strictly comparable because the new score uses the full
training pattern and calibration. The new initial period estimate is
479.970474 samples (−61.5 ppm); the timing loop ends at 480.204299 samples.
These are receiver estimates influenced by channel transitions, not independently
measured hardware clock rates. Median carrier RMS powers are 0.002003143 and
0.001902732, giving an E1/E0 decision threshold of 0.94987.

Recovered payload SHA256:
`2bcb0e3e0b524a5f1e09bb8ef22e96cad3ee50073d7664c555368fcc95dca332`.
It matches both the reference packet and the frozen
`test-vectors/physical-hello-v0.txt` fixture byte for byte.
Detailed outputs are in `measurements/physical.txt`, `physical-symbols.csv`,
`physical-no-tracking.txt` and `physical-full-symbols.txt`.

Fixture SHA256 values for identification:

- `transmission.wav`: `b12d8e6cf0b6e3db822be9e1906e35bb673a6e0e77732fcde64045bccf348e48`
- `recording.wav`: `1b1032bc9f6de390f3f863d390c41b036cc23abb8f4e49d9422b8f204feb1255`

## 12. Current practical limits

The CLI accepts 48 kHz mono 16-bit PCM only. It is an offline, in-memory receiver,
not a live audio capture/playback or streaming implementation. Memory scales
with recording length; packet extraction itself is bounded to 280 bytes.
The complete preamble must be available for reliable acquisition. Missing
payload information, severe clipping and destructive carrier nulls remain errors.

The clock estimator assumes the current continuous-phase, integer-cycle V0
transmitter and a common, approximately constant sample-clock ratio. The timing
loop accommodates gradual phase movement, but nonlinear resampling, dropped
samples, independent large carrier offsets and Doppler were not characterized.
Coarse acquisition still loses trials under sufficiently strong bias plus noise.
Strict preamble/header checks can reject a frame before CRC is reached.

AWGN and a single echo are useful controls, not comprehensive room or microphone
models. Speech/music interference, colored noise, multiple long reflections,
automatic gain control, noise suppression, codecs, hardware frequency changes
during a packet and repeated physical transfers remain unmeasured. Several
symbols in the successful recording have confidence below 0.5, so it should not
be treated as having a proven large physical margin.

## 13. Recommended next milestone

Run a controlled physical campaign before changing bitrate. Keep the same
17-byte payload initially, capture raw 48 kHz mono 16-bit PCM, disable voice
enhancement if possible, and avoid clipping. Record before transmission starts
and leave a small tail; do not trim or align captures manually.

Collect at least 30 separate captures at each of 20, 35 and 50 cm in a quiet room,
logging speaker/microphone model, volume, orientation, distance and processing
settings. Then collect 100 transfers at the chosen baseline arrangement and seek
100/100 CRC-valid, byte-identical recoveries. Save failed captures as well as
successful ones. Repeating the decoder on one capture is not a repeated physical
trial. Run:

```powershell
./lab/physical-link.ps1 -Reference transmission.wav -RecordingsDirectory ./captures
```

The script creates a fresh results directory with per-recording diagnostics,
symbol CSVs, BER/missing-bit counts and a CRC-plus-reference summary. Separate
results by distance/hardware. After establishing repeatability, test maximum
payloads, longer distances and additional rooms. Use the symbol traces to decide
whether remaining failures come from acquisition, timing, stationary channel
bias, bursts or room decay.

## 14. Protocol and future codec recommendations

No on-air revision was needed. V0 still emits wire version 1 with the same
preamble, sync, fields, CRC and symbol waveform. Receivers now explicitly reject
unknown versions and nonzero reserved flags; encoders reject those unsupported
values too. This tightens validation without changing valid existing V0 frames.

The present preamble provides enough training for the measured clock and gain
estimates. If later physical measurements justify it, a versioned, less repetitive
training sequence with isolated carrier runs could distinguish room decay from
carrier gain and help harder timing acquisition. That requires comparative
measurements before changing the wire format.

FEC is not implemented or used to explain away receiver errors. Once the repeated
physical campaign identifies the remaining error distribution, evaluate a
separate codec/interleaving layer against recorded channels, retaining an outer
CRC. There is no measured justification in this pass for selecting a particular
code or increasing bitrate.

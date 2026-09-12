# Phone → PC physical investigation — 2026-09-09

Both supplied multi-frame recordings now recover the complete 17-byte file, with
zero bit differences in both frames, valid packet CRCs and the transmitted file
SHA-256 verified. The successful legacy recording still has 328/328 bits, BER 0
and an exact payload match. No captures, transmitted waveforms, protocol fields,
bitrate, guard durations or existing Phase 3 work were replaced. No commit was made.

The public reproducible evidence is in
[measurements/phone-pc-2026-09-09](measurements/phone-pc-2026-09-09/).
`before/` and `after/` contain derived per-frame/per-symbol CSVs and channel
measurements. Raw command logs, validation logs, the original baseline executable,
and recordings remain in the private development archive. The recordings are
identified publicly by [SHA-256](measurements/phone-pc-2026-09-09/sha256.csv).

**1. Exact receiver cause.** The energy receiver assumed that the ratio measured
on the alternating preamble remained a suitable, memoryless decision boundary
throughout a packet. The recordings violate that assumption: carrier observations
depend on preceding symbols, and the complex channel response changes within a
frame. There was a second, independently identifiable acquisition defect: a
qualifying raw candidate could disappear when gain calibration reduced its sync
matches below the existing limit. This explains the missing second candidate in
`phone-pc-01.wav`. These are receiver causes established from samples and code;
the recordings alone cannot uniquely assign the physical response to room echoes,
speaker processing, microphone AGC or phone playback processing.

**2. Evidence and measurement limits.** Independent 240-sample complex correlators
were applied to the original PCM. In recording 01 metadata, median desired
2200 Hz power changes from 0.006638 in bits 0–63 to 0.001333 in bits 256–319;
1200 Hz changes from 0.002107 to 0.001391. The unchanged receiver threshold is
3.144001. The data frame has an even larger preamble ratio, 5.396791, followed by
much weaker 2200 Hz responses. These block values also depend on bit history:
they must not be interpreted as pure hardware gain measurements.

Complex channel fits provide additional evidence for memory. For recording 02
DATA at 2200 Hz, a memoryless fit has R² 0.675; adding two delayed symbols raises
it to 0.938. A model trained only on the known training sequence predicts the
payload with R² 0.909. For legacy, the memoryless R² is already 0.964. On recording
01 metadata, the corresponding memoryless/all-frame fit is only 0.394, and even
an eight-delay model frozen after training has negative payload R². A constant
gain/phase calibration does not generalize there. The fitted delayed-symbol
coefficients establish a useful impairment model, not a unique acoustic impulse
response. See the `*-channel.json` files and `lab/channel-response.py`.

**3. Per-frame identification and results.** Candidate and bit indices are zero
based; times are receiver estimates, not independently measured propagation
delays. Final CRC-valid headers establish identities, including the two initially
corrupt IDs. Order alone was never used to assign a trustworthy identity.

| Recording / frame | Start sample; seconds | Original reception | Diagnostic full energy decisions | Final reception |
|---|---:|---|---|---|
| 01 / metadata | 79680; 1.660000 | 53/664 errors; CRC fail | Same 53/664, BER 0.079819277 | 0/664; CRC F33C9CE9 pass |
| 01 / data | 408000; 8.500000 | Not acquired: no received bits | 53/328, BER 0.161585366 at the independently qualifying raw candidate | 0/328; CRC 81166FAF pass |
| 02 / metadata | 72288; 1.506000 | 4/160 errors; 504 unobserved; **invalid preamble** | 11/664, BER 0.016566265 | 0/664; CRC F33C9CE9 pass |
| 02 / data | 400584; 8.345500 | 0/328; CRC pass | No extension needed | 0/328; CRC 81166FAF pass |
| Legacy | 23136; 0.482000 | 0/328; CRC pass | No extension needed | 0/328; CRC 945C24C0 pass |

The probes continue the original energy decisions to an explicitly selected
reference **length**. They do not supply reference bit values to demodulation,
are labelled `energy_probe`, and cannot declare reception success. Recording 01
DATA's probe uses the restored raw candidate; the old receiver did not expose
that frame at all. Recording 02 metadata's original BER is 4/160 = 0.025 on
observed bits, not 11/664; the latter is an additional diagnostic measurement.

Recording 01 metadata's original untrusted ID was `2BCB063E`; its received CRC
was `713C1CE9` and calculated CRC `02706AFA`. Recording 02 metadata's original
untrusted ID was `ABCB0E3E`; CRC was **not reached**, rather than tested and failed.
After correction every header has ID `2BCB0E3E`, sequence 0 and the expected kind.

All 53 original metadata error positions in recording 01 are:
`116,166,172,199,259,263,295,308,316,322,332,342,353,363,372,388,398,411,420,430,441,453,458,473,477,480,485,498,508,509,516,520,523,536,542,543,546,547,550,556,557,569,573,585,593,602,609,617,625,628,632,638,648`.
Recording 02 metadata's original observed errors are `40,67,76,96`; continuing
the energy receiver adds `177,462,503,558,560,563,574`. All data-probe error
positions and every symbol's confidence/powers/timing are in
[the probe CSV](measurements/phone-pc-2026-09-09/before/multi01-data-probe-symbols.csv).
Every final error-position list is empty; all final missing-bit counts are zero.

| Frame | Acquisition preamble / sync | Quality; concentration | P1200 / P2200 | Energy ratio |
|---|---:|---:|---:|---:|
| 01 metadata | 64 / 16 | 0.757081; 0.770241 | 0.002123428 / 0.006676059 | 3.144001 |
| 01 data, restored raw | 64 / 16 | 0.631803; 0.795499 | 0.001406689 / 0.007591606 | 5.396791 |
| 02 metadata | 63 / 15 | 0.720741; 0.609405 | 0.004213853 / 0.003278093 | 0.777932 |
| 02 data | 64 / 16 | 0.774398; 0.663221 | 0.003770879 / 0.004048002 | 1.073490 |
| Legacy | 64 / 16 | 0.890677; 0.827405 | 0.018478976 / 0.061604288 | 3.333750 |

Each four-block energy-threshold profile is constant at the listed ratio.
Preamble energy-decision training errors before/after the old profile fitting
are 0/0 except recording 02 metadata, 1/1. The equalizer has a different decision
rule: its displayed energy ratio is a retained measurement, not its classifier.

**4. Why legacy succeeds.** The recording has a substantially stronger and more
stationary observed response. Active-frame RMS is 0.194174, compared with
0.052364–0.061562 in the multi-frame recordings. Its direct complex channel
component dominates; the energy receiver has no low-confidence symbols. It
continues to use the original decoding path, without invoking the equalizer.
Thus the same physical equipment can produce a clean frame, but these files do
not demonstrate identical channel response or captured signal level.

**5. Why the multi-frame recordings failed.** Recording 01's metadata has errors
distributed from bit 116 through bit 648, not just a damaged leading edge. Its
second packet suffers an additional acquisition rejection: calibrated refinement
scores **64 preamble / 12 sync / quality 0.723355**, whereas raw scoring gives
64/16 and quality 0.631803. Twelve sync matches fail the unchanged requirement
of thirteen. The subsequent memoryless data decisions are also poor. Recording
02 detects both candidates, but metadata stops at the invalid preamble; its
DATA packet is exact. Consequently reassembly correctly reports missing metadata.

**6. Metadata length and patterns.** The clean transmitter was inspected directly.

| Frame | Payload bytes | Total bytes / bits | Modulated duration | Transitions | Longest constant run |
|---|---:|---:|---:|---:|---:|
| Metadata | 59 (50 fixed + 9 filename) | 83 / 664 | 6.64 s | 314 | 59 symbols |
| DATA | 17 | 41 / 328 | 3.28 s | 186 | 28 symbols |
| Legacy | 17 | 41 / 328 | 3.28 s | 185 | 28 symbols |

Metadata has 2.024 times the exposure and a 590 ms constant run. This can increase
uncoded packet error probability and exposes calibration learned on alternation
to different symbol histories. It is not a metadata parser bug or a length-only
explanation: recording 02 fails before reaching most payload bits, and recording
01's short DATA is also impaired. Two multi-frame trials cannot establish an
independent-bit error rate, a population PER or a statistical first-frame effect.

**7. Startup and waveform start conditions.** The clean metadata starts at sample
9600 (0.20 s), DATA at 337920 (7.04 s). The first 80 symbols of metadata, DATA and
legacy are sample-for-sample identical in the PCM references. All packets start
with the same modulation phase; each is independently framed. Legacy has no
added edge silence; multi-frame has 200 ms leading silence. Recording 02's
training error at bit 40 is approximately 400 ms into the packet, not evidence
of a simply clipped-off first symbol. Its initial channel fit is less stationary
than DATA's. Startup/channel variation is significant to calibration, but a
specific AGC or phone-startup mechanism cannot be isolated from these recordings.
No transmitter preconditioning was added.

**8. Guard and scanner.** The reference contains exactly 9600 zero samples before,
between and after the frames, and totals 504960 samples / 10.52 s. Recorded
candidate spacing implies gaps of approximately 200.167 ms and 199.500 ms.
Gap first/second-half RMS is 0.014965/0.013755 in recording 01 and
0.013083/0.008798 in recording 02, versus active RMS around 0.05–0.06. Residual
sound/noise exists in the guard; it does not explain a defect that disappears
with receiver-only changes on the identical WAV. Both files fit inside one
offline scan window, so window overlap/ownership cannot explain the missing
candidate. Scanner logic and the 200 ms guard remain unchanged.

**9. Clock and timing.** Initial estimates are −24.720 ppm (01 metadata),
+33.325 ppm (01 DATA), no accepted estimate / nominal period (02 metadata),
−11.168 ppm (02 DATA), and −9.481 ppm (legacy). Disabling timing tracking leaves
51 errors in 01 metadata; full-symbol decisions leave 51 and introduce two DATA
errors in recording 02. Disabling clock estimation still leaves 54 metadata
errors in recording 01, although it also exposes its second bad candidate.
These controls reject simple accumulated symbol-clock drift as the main cause.
Phase drift still matters to a coherent channel model: startup channel phase
changes can bias a preamble clock estimate. The equalizer therefore compares
the estimated and nominal clock priors using held-out training response, before
reading the header. It selects the measured period for 01 metadata and nominal
480 for 01 DATA and 02 metadata. No payload/CRC-guided clock search is performed.

**10. Calibration, gain and clipping.** Whole-recording RMS/peak are
0.047534/0.264992 (01), 0.051393/0.307321 (02), and 0.153526/0.523087 (legacy).
There are zero PCM rail samples. This excludes digital saturation at the recorded
PCM rails, not upstream compression or speaker distortion. A large preamble
carrier ratio is not itself a defect: legacy succeeds at 3.33375. The failing
assumption is that this one ratio describes subsequent symbols despite memory
and response changes. Disabling carrier calibration does not repair recording
02 metadata. Frozen channel fits and the time-resolved powers justify local
decision-directed adaptation rather than a new fixed threshold.

**11. Files changed for this investigation.** Production changes are
`receiver.rs`, new `receiver/equalizer.rs`, `demodulation/acquisition.rs`, and the
corresponding acquisition call in `demodulation/sync.rs`. Tooling changes are
new CLI `frame_diagnostics.rs`, CLI `main.rs`/`evaluate.rs`, `simulation.rs`, and
new `lab/channel-response.py`. Tests are new `tests/channel_memory.rs` and additions
to CLI `tests/cli.rs`. README, this report and its measurement directory document
the change. Other dirty Phase 3 files predated this investigation and were kept.

**12. Algorithmic change.** The ordinary energy decode runs first and every
existing valid packet is retained. A failed candidate can receive one additional
decode using a model selected entirely from the fixed 64-bit preamble and 16-bit
sync. For each carrier, complex observations are modelled as the sum of direct
and delayed carrier-presence responses. Orders 0–8 delayed symbols are fitted
with four robust weighted least-squares iterations and a 0.1 ridge. Five
interleaved held-out folds choose the order and one of the two clock priors.
Selection must observe all 80 training bits correctly using actual feedback
decisions, with normalized validation error below 1. No training symbol is
replaced with its expected value.

Each subsequent bit minimizes the two-carrier, variance-normalized complex
residual cost using previous detected bits. Confident decisions update the
channel coefficients with normalized LMS step 0.1; low-confidence decisions do
not update them. Training ends at bit 80. Selected orders are 7, 5 and 1 for the
three formerly unrecoverable frames, with validation errors 0.039789, 0.094585
and 0.626441. Failed retries cannot expose a packet: unchanged exact framing and
CRC validation remain mandatory. A trained but truncated retry is retained as
incomplete, so the incremental adapter waits for the remaining audio instead of
consuming the energy path's invalid prefix. Physical replay initially exposed
this integration issue; replaying all three captures in 250 ms chunks now
recovers every frame with exact bits. This is replay validation, not a live
speaker/microphone duplex experiment. A previously qualifying raw acquisition is
retained when calibrated refinement rejects it, subject to the same 56/64,
13/16, quality 0.5 and concentration 0.04 requirements.

The general approach of training an equalizer and then updating it from detected
symbols is established decision-feedback practice; see the primary
[MathWorks equalizer documentation](https://www.mathworks.com/help/comm/ug/adaptive-equalizers.html).
Tonequill's implementation is a bounded two-carrier channel predictor, not a
port of that library. The evidence for needing it comes from these PCM measurements
and the synthetic tests, not that external documentation.

**13. Synthetic reproduction.** `simulation::Channel` now supports additional
delayed paths with independently changing amplitudes. The asserted regression
uses 10, 20 and 40 ms paths, gains 0.35 or 0.5 / 0.3 / 0.12, relative endpoint
path gains 0.8 / 1.1 / 0.7, a 2200 Hz amplitude ramp from 2.0 to 0.6 or 1.0 over
400 symbols, and 20 dB broadband SNR. Seeds 1/17/42 generate different binary
payloads and noise; clock mismatch is −80/+33/+120 ppm. Payload lengths are
17/59/128/256. **All 48 old cases fail; all 48 new cases match every bit and the
packet.** No physical WAV or hello payload is input to this regression.
The [CSV](measurements/phone-pc-2026-09-09/synthetic-grid.csv) records the matrix.
An additional test covers offline and incremental decoding of independently
framed metadata and DATA with precisely
200 ms between transmission boundaries, leading/trailing silence, and deliberate
CRC corruption on either side. Both valid frames recover; deliberately corrupt
frames remain rejected. More severe echo configurations explored during diagnosis
still fail; this is a demonstrated impairment region, not a universal echo guarantee.

**14. Actual before/after offline decoding.** The exact release CLI commands now
report 2 found / 2 CRC-valid / 0 rejected for each multi-frame capture, one data
packet expected and recovered, 17 bytes and final SHA-256 OK. Previously they
reported 1/0/1 and 2/1/1 respectively and left the output untouched. Legacy is
unchanged at 1/1/0. The original command logs are retained in the private
development archive.

**15. BER and confidence after correction.** All five final frames have BER 0,
no missing bits, and exact reference packet matches. Low-confidence counts
(confidence < 0.5) are 22 and 3 for recording 01, 21 and 53 for recording 02,
and 0 for legacy. The original energy counts were 162 for 01 metadata, 24 on
the observed 160 bits of 02 metadata, 53 for 02 DATA, and 0 for legacy. Confidence
is a decision contrast rather than a calibrated probability; complex residual
contrast and energy contrast should not be treated as interchangeable SNRs.

**16. Do both multi-frame captures fully decode?** Yes. Outputs are
`target/physical/received-01.txt` and `target/physical/multi-received-02.txt`.
Their SHA-256 is
`2bcb0e3e0b524a5f1e09bb8ef22e96cad3ee50073d7664c555368fcc95dca332`,
identical to the original and the transmitted metadata hash.

**17. Is legacy still exact?** Yes: 328/328, zero errors/missing bits, CRC
`945C24C0`, exact packet match, zero low-confidence symbols, and the same file
SHA-256. Its final period is still 479.987027 samples/symbol. The legacy format
has no transmitted SHA-256; its payload was compared with the known source.

**18. Checks and integrity tests.** Before PHY edits: fmt/check/test/clippy passed,
with 91 normal tests and 6 optional tests skipped. Final `cargo fmt --check`,
`cargo check`, `cargo test`, and
`cargo clippy --workspace --all-targets -- -D warnings` all pass: **96 normal
tests, zero failures, 7 optional tests skipped in the ordinary run**. Running
`cargo test --release --workspace -- --ignored` executes all **7 optional tests
successfully**, including both historical physical recordings, all three new
captures (offline and 250 ms incremental replay), exhaustive lengths/alignments,
noise-only acquisition, 10 KiB WAV and
1/10 KiB virtual duplex sample transfer. The new diagnostics test checks corrupt
IDs, partial observations, explicit probes, CSV shape, and no output creation
or overwrite with missing metadata. Another test flips each of the 80 strongly
transmitted training bits separately and requires rejection. Existing CRC,
missing-data, conflicting-metadata and final-SHA failure assertions remain intact.

**19. Remaining physical limitations.** There are only two supplied multi-frame
trials and one new legacy control, all used during development. They establish
these regressions, not success rates for unseen phones, rooms or distances.
Initial acquisition must still qualify; severe interference, deep fades, rapid
response changes, large coherent phase errors or memory beyond the fitted range
can prevent recovery. Decision feedback can propagate errors. All state/model
sizes are bounded and the original valid-packet path is retained, but physical
live duplex behavior has not been demonstrated by these one-way captures.

**20. Further PHY work versus retransmission.** The original behavior is evidence
of a systematic receiver assumption failure, not enough evidence to classify the
metadata losses as ordinary independent random errors. The targeted receiver fix
is justified by that evidence and the 48 synthetic reproductions. There are no
residual errors in the supplied captures after correction. Do not redesign the
PHY further on this sample alone: collect independent repeated trials and run
the existing Phase 3 live duplex procedure. Use metadata/data retransmission for
remaining occasional frame losses, while investigating any newly reproducible
systematic class. Offline decoding must continue to fail if any required frame
or final hash is unavailable; it has no feedback to repair a missing transmission.

Run from the repository root to reproduce the final checks and actual outputs:

```powershell
cargo fmt --check
cargo check
cargo test
cargo clippy --workspace --all-targets -- -D warnings
cargo test --release --workspace -- --ignored

cargo run --release -- decode captures/phone-pc-01.wav target/physical/received-01.txt
cargo run --release -- decode captures/phone-pc-multi-02.wav target/physical/multi-received-02.txt
cargo run --release -- decode captures/phone-pc-legacy-01.wav target/physical/legacy-received-01.txt

New-Item -ItemType Directory -Force target/phone-check | Out-Null
cargo run --release -- diagnose-frames target/physical/tx-hello.wav captures/phone-pc-01.wav --frames-csv target/phone-check/frames.csv --symbols-csv target/phone-check/symbols.csv
.venv/Scripts/python.exe lab/channel-response.py captures/phone-pc-01.wav target/phone-check/frames.csv target/phone-check/symbols.csv target/phone-check/channel.json

# A/B: original energy receiver and original acquisition behavior.
cargo run --release -- diagnose-frames target/physical/tx-hello.wav captures/phone-pc-multi-02.wav --no-equalizer
# Additional measurement beyond the invalid prefix; expected nonzero exit.
cargo run --release -- diagnose-frames target/physical/tx-hello.wav captures/phone-pc-multi-02.wav --no-equalizer --candidate 0 --reference-frame 0 --probe-energy
```

The Python measurement helper requires NumPy (available in the existing `.venv`).
Frame CSVs have 36 columns, symbol CSVs 12. Original baseline CSVs retain their
earlier 31-column schema. CRC fields report unavailable checks as the actual
framing failure; a failed preamble must not be relabelled as a checked CRC failure.

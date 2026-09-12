# Follow-up: time-varying carrier response during startup

7 September 2026. The recording `captures/prova-20cm.wav` now recovers all 328
frame bits with zero observed errors and a valid CRC. Its recovered 17-byte
payload matches the frozen `test-vectors/physical-hello-v0.txt` fixture:

`2bcb0e3e0b524a5f1e09bb8ef22e96cad3ee50073d7664c555368fcc95dca332`

The WAV SHA256 is
`c69dc8303b6412d98e36ae834cfa0d19d4a17b4d86db95f922fdd25dec8af275`.
It is byte-for-byte identical to the file previously named `captures/35cm.wav`.
Those names therefore represent one capture, not independent distance trials.
The earlier root `recording.wav` is a separate capture and continues to pass.

## Failure and measurement

Before this change, acquisition found the transmission at sample 36600, with
62/64 full-window preamble matches and 16/16 sync matches. A single calibrated
E1/E0 threshold of 8.73054 produced six preamble errors in the first 160 decoded
bits. The parser rejected the preamble and did not decode the remaining 168 bits.

The received carrier powers were not stationary during training. Four local
carrier-power ratios were approximately 3.57, 6.39, 10.00 and 10.89. Applying the
global ratio to weak early symbols overcompensated for the later carrier bias.
This is consistent with startup changes in gain or spectral response; the exact
hardware or recording processing responsible has not been established.

A first experiment learned one replacement threshold from labelled preamble
decisions. That fixed the preamble but still produced a CRC-bit error. It was
discarded rather than accepting a failed CRC or tuning a threshold to that bit.

## Final receiver change

The receiver retains its global median carrier-power ratio as the default. When
that ratio misclassifies known preamble symbols, it also evaluates a startup
profile from four contiguous 16-symbol blocks. Each block contains eight known
observations of each carrier. Ratios are interpolated in log space between block
centers; the final block's ratio is held for the remaining frame.

The local profile is selected only if it strictly reduces preamble decision
errors compared with the global ratio. Otherwise the global estimate is kept.
Timing recovery still uses the original physical carrier-power calibration;
classification now exposes its actual threshold separately in each symbol's
metrics. Acquisition scores retain their original meaning and are labelled as
acquisition measurements in diagnostics.

On this capture, training errors fall from six to zero. The held decision ratio
is 10.89243, and the complete frame subsequently passes CRC and matches the
reference. Neither payload bits nor CRC values are used to choose calibration.
Observed preamble bits are never replaced by expected labels. The protocol,
modulation, packet parser and CRC acceptance criteria are unchanged.

Changed code: core `demodulation/acquisition.rs`, `receiver.rs`, the `sync.rs`
compatibility call, and `simulation.rs`; CLI `main.rs`, `evaluate.rs` and
`diagnostics.rs`; core receiver tests and CLI physical-fixture tests. No new
dependencies were added. `ReceiverOptions::train_decision_threshold` defaults to
true; `--power-ratio-threshold` on diagnose/evaluate reproduces the earlier
global-threshold behavior. The CSV adds `decision_ratio` after existing columns.

## Independent synthetic A/B evidence

The channel harness now also supports a relative-carrier startup ramp. This is a
controlled model, not an assertion about the physical recording's exact channel.

The following fixed setup uses 200 seeded random 17-byte payloads per SNR:
48 kHz, gain 0.1, 2200 Hz amplitude gain increasing from 0.25 to 3 over 64 symbols,
an 8 ms echo of amplitude 0.3, initial phase 1.3 radians, and 6037/2111 samples of
leading/trailing padding. Noise SNR retains the original active-waveform,
broadband definition.

| SNR | Global threshold: valid / trials | Startup profile: valid / trials | Startup-profile errors / observed bits |
|---|---:|---:|---:|
| 15 dB | 1/200 | 200/200 | 0/65,600 |
| 10 dB | 20/200 | 200/200 | 0/65,600 |
| 5 dB | 22/200 | 200/200 | 0/65,600 |

The old threshold's non-monotonic results reflect calibration failure; noise
changes estimated powers and can accidentally move the decision boundary.
Higher SNR alone could not fix that model error.

A separate common-gain ramp experiment (gain 0.03 increasing 8x over the packet,
4x carrier bias, 137-sample echo of amplitude 0.3) improved from 90 to 91 valid
packets out of 100 at 10 dB, and from 11 to 18 at 5 dB. Both versions recovered
100/100 at 15 dB. This difficult case remains far from reliable at 5 dB.

The complete original eleven-file sweep was rerun into
`measurements/calibration/regression`. No scenario lost valid packets. Default
AWGN at -10 dB improved from 143/200 to 144/200; the other valid-packet counts
were unchanged. Observed bit-error counts within already failing packets can
worsen: default AWGN at -15 dB changed from 42 to 52, and combined -10 dB from
768 to 773, with zero valid packets in both versions. Missing-bit counts remain
separate from BER. No incorrect CRC-valid packets were observed.

Results and physical symbol traces are saved under `measurements/calibration`.
The earlier executable is retained locally under `target/calibration-baseline`
for reference; the supported ablation flag is the reproducible comparison path.

## Reproduce

```powershell
cargo run --release -- diagnose target/verifica/tx.wav captures/prova-20cm.wav --limit 0
cargo run --release -- diagnose target/verifica/tx.wav captures/prova-20cm.wav --limit 0 --power-ratio-threshold

$trialArguments = @('evaluate', '--trials', '200', '--snr-db', '15,10,5',
    '--gain', '0.1', '--one-carrier-start-gain', '0.25', '--one-carrier-gain', '3',
    '--carrier-ramp-symbols', '64', '--echo-delay-samples', '384', '--echo-gain', '0.3')
& ./target/release/tonequill.exe @trialArguments --power-ratio-threshold
& ./target/release/tonequill.exe @trialArguments

./lab/evaluate.ps1 -OutputDirectory target/startup-regression
cargo test --release -p tonequill-core --test receiver -- --ignored
cargo test -p tonequill-cli --test cli -- --ignored
```

The global-threshold physical diagnosis is expected to exit with failure.

## Verification and remaining limits

All 29 normal tests and four optional tests passed. Added tests cover multiple
startup amplitudes/seeds with noise and echo, rejection of physically flipped
preamble symbols, and both diagnosis and exact payload output from the new local
fixture. The 257-length/480-offset sweep and 200 noise-only trials still pass.
`cargo fmt`, `cargo check`, `cargo test` and strict all-target Clippy are clean.

Strong echo can still prevent acquisition before calibration runs. In a small
20-trial grid with carrier gain ending at 3 and the same startup ramp, increasing
the 8 ms echo from 0.3 to 0.5 caused all trials to fail acquisition with either
receiver. This change does not address that separate limit. It also assumes the
relative carrier response becomes approximately stable by the end of training;
later spectral changes remain uncharacterized. Low-confidence symbols persist
in the successful physical recording, so repeated fresh air-channel captures
are still needed before claiming reliable operation at any distance.

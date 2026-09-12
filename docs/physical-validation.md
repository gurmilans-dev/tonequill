# Physical validation

This page separates results measured through real speakers and microphones from
software-only tests. Raw microphone WAV files remain private because they can
contain environmental audio. Public evidence consists of hashes, derived
measurements, and reproducible commands.

## Validated paths

| Path | Result | Scope |
| --- | --- | --- |
| Phone speaker → air → PC microphone → CLI one-way receiver, 17-byte text | File committed after SHA-256 verification in repeated live sessions | One phone/PC setup in an indoor room |
| Phone speaker → air → PC microphone → CLI one-way receiver, 257-byte binary file | Both data packets and metadata reconstructed; one successful session required replay | Same hardware class and environment |
| Saved physical microphone captures → offline receiver | Legacy and multi-frame captures decode with valid CRCs and exact output hashes | Three retained private recordings |
| Saved physical captures → incremental streaming receiver | Frames recover when replayed in 250 ms chunks | Replay test; it does not exercise an audio device |

The 17-byte reference payload has SHA-256
`2bcb0e3e0b524a5f1e09bb8ef22e96cad3ee50073d7664c555368fcc95dca332`.
It is retained byte-for-byte as
[physical-hello-v0.txt](../test-vectors/physical-hello-v0.txt); its text contains
the former SonicLink project name because changing it would invalidate the
physical-capture regression and its published hashes.
The physical-capture hashes are recorded in
[the 2026-09-09 measurement set](measurements/phone-pc-2026-09-09/sha256.csv)
without publishing the audio itself.

## Interpretation

The tests establish end-to-end acoustic correctness on the tested setup. They do
not establish a guaranteed range or packet error rate across devices. Informal
tests were usable around 60–70 cm, while results varied farther away; distance,
orientation, room reflections, speaker response, microphone processing, and
Windows audio discontinuities were not controlled independently.

Repeated playback in a single one-way receive session is supported: already valid
packets are retained, duplicates are ignored, and a later replay can supply a
missing packet. Completion occurs only after all packets are present and the
whole-file SHA-256 matches.

## Not yet physically validated

- the complete transfer through the Tauri desktop UI;
- feedback, acknowledgements, and retransmissions between two acoustic endpoints;
- macOS or Linux live audio;
- a controlled distance or multi-device performance campaign.

Reliable duplex is covered by deterministic software campaigns, including loss,
delay, jitter, corruption, and duplication. Those results support protocol
behavior only; they are not a substitute for a two-device acoustic experiment.

For the receiver changes derived from the private captures, see the
[physical investigation](phone-pc-physical-investigation.md). For the earlier
receiver baseline, see the [engineering report](receiver-engineering-report.md).

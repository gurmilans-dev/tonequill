# Contributing to Tonequill

Tonequill is an experimental acoustic data link. Changes should preserve the
Robust100 profile unless a compatibility decision is documented explicitly.

Before opening a pull request:

1. explain the problem and the observable behavior of the change;
2. keep generated audio, microphone captures, build output, and local device
   details out of the commit;
3. add focused tests for protocol, receiver, or transfer behavior changes;
4. run the quality gates below.

```powershell
cargo fmt --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

npm --prefix apps/desktop ci
npm --prefix apps/desktop run contracts:check
npm --prefix apps/desktop run format:check
npm --prefix apps/desktop run typecheck
npm --prefix apps/desktop run lint
npm --prefix apps/desktop test
npm --prefix apps/desktop run build
```

Physical results need enough context to interpret them: hardware category,
distance, room conditions, playback method, sample rate, exact command, and file
hash. Publish derived measurements when possible. Treat raw microphone recordings
as private because they can contain environmental audio.

Use a concise pull-request description that states what changed, why, and which
checks passed. Do not include secrets, personal paths, device identifiers, or
unrelated generated files.

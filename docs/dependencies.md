# Dependency and supply-chain review

Review date: 12 September 2026.

Tonequill pins the Rust toolchain in `rust-toolchain.toml`, the Node major version
in `.node-version`, Rust dependencies in `Cargo.lock`, and npm dependencies in
`apps/desktop/package-lock.json`.

## Automated checks

| Check | Result |
| --- | --- |
| `npm audit --package-lock-only` | 0 known vulnerabilities at every severity |
| `npm ci --ignore-scripts --dry-run` | Passed |
| Cargo resolved-package license metadata | Present for every third-party crate |
| `cargo audit` | Not run: `cargo-audit` was not installed in the review environment |

The missing `cargo audit` result is a release-review limitation, not evidence
that the Rust dependency graph has no advisories. Run it before publication when
the tool is available:

```powershell
cargo install cargo-audit --locked
cargo audit
```

## License review

The Rust graph is primarily MIT and Apache-2.0 licensed. Some transitive crates
use MPL-2.0, and `r-efi` offers a permissive MIT or Apache-2.0 option among its
alternatives. The npm graph is primarily MIT, Apache-2.0, ISC, and BSD licensed;
metadata packages also include CC0-1.0 and CC-BY-4.0 data. No dependency license
identified in this review conflicts with distributing Tonequill under MIT.

This is an engineering inventory rather than legal advice. Re-run the inventory
and vulnerability checks whenever either lockfile changes.

## Update policy

Dependency upgrades should be narrow and tested. Avoid changing the PHY or live
audio behavior solely to reduce duplicate transitive versions. For release
review, inspect the lockfile diff, run the full software quality gates, and keep
Tauri permissions and the content-security policy as small as the application
allows.

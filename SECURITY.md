# Security policy

## Supported version

Security fixes currently target the latest revision of the `main` branch. No
stable release series is maintained yet.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting form in this repository's
**Security** tab when it is available. If it is unavailable, open an issue that
asks the repository maintainers for a private contact channel without including
exploit details, secrets, personal data, or captured audio.

Include the affected revision, expected impact, reproduction conditions, and the
smallest safe proof of concept. Please allow time for triage before public
disclosure.

Tonequill does not provide encryption, authentication, or peer identity. CRC32
and SHA-256 checks detect corruption; they do not make an acoustic transfer
confidential or prove who sent it.

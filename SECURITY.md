# Security policy

## Reporting a vulnerability

Please report vulnerabilities **privately** through GitHub:
<https://github.com/SpryDynamics/nastro-rs/security/advisories/new>

Do not open a public issue for a security problem. Include what you found, how to reproduce it, and the version
(`nastro --version`). We aim to acknowledge reports within a few working days.

## Scope

nastro records your system audio and microphone to local files and spawns `ffmpeg`. Of particular interest:
anything that makes recordings readable by other users, writes outside the recordings directory, runs unintended
commands, or weakens the integrity of the signed release artifacts.

## Supported versions

Only the latest release receives fixes while the project is pre-1.0.

## Verifying a download

Release archives come with `SHA256SUMS`, and the Windows binaries are Authenticode-signed. See
[docs/releasing.md](docs/releasing.md#verifying-a-release-artifact-yourself).

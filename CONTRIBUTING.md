# Contributing to nastro-rs

Thanks for your interest. This is a small project; the notes below keep things smooth.

## How this repository is run

The source of truth is a private Azure DevOps repository at Spry Dynamics, LLC. This GitHub repository is a
**read-only mirror** of it, and a few things follow from that:

- **Issues** opened here are copied into the maintainers' private tracker, and your comments on them are
  copied too. Maintainers' replies appear here as "Maintainer update" from the `sprydynamics-nastro-sync` bot. Internal
  discussion is never published. **Do not put recordings, transcripts, meeting names, credentials or other
  private data in an issue or comment.**
- **Pull requests** are welcome, but they are not merged on GitHub: the mirror only ever fast-forwards from Azure
  DevOps. A maintainer ports an accepted change by hand, credits you with a `Co-authored-by:` trailer, and closes
  your PR with a link to the resulting commit. Small, focused PRs are the easiest to port.
- Direct pushes to `main` are blocked.

## Before you open a PR

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Build requirements are in the [README](README.md#requirements) (on Linux: `libpulse-dev`; at runtime: `ffmpeg`).
CI runs the same checks on Windows and Linux, plus a dependency license/advisory gate (`cargo deny`, see
`deny.toml`). A new dependency must be MIT/Apache-2.0/BSD-style compatible with this project's MIT license.

Behaviour changes should come with tests and a line in `CHANGELOG.md`. Platform-specific code (WASAPI, PulseAudio,
console signals) is hard to test in CI; say in the PR how you verified it, and on what OS.

## Sign your commits (DCO)

Every commit must carry a `Signed-off-by:` line, which you add with `git commit -s`. It certifies the
[Developer Certificate of Origin 1.1](https://developercertificate.org/): that you wrote the change, or otherwise
have the right to submit it under this project's license. Cryptographically *signing* commits (SSH or GPG) is
appreciated but not required.

## License

By contributing you agree that your contribution is licensed under the [MIT License](LICENSE), the same as the
project. Copyright in the project as a whole is held by Spry Dynamics, LLC; you keep copyright in your own
contributions.

## Reporting security problems

Not here: see [SECURITY.md](SECURITY.md).

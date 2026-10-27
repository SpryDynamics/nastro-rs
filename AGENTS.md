# AGENTS.md

Guidance for AI coding agents working in this repository. `README.md`, `CONTRIBUTING.md` and `docs/releasing.md` hold the
full story; this file is the short list of things that are easy to get wrong.

## What this is

`nastro-rs` is an unofficial Rust reimplementation (Windows + Linux) of a terminal call recorder. The command is `nastro`.
It records system audio plus microphone to `.m4a`, and transcribes on-device with an embedded whisper.cpp (optionally
WhisperX for speaker labels). Do not copy code, images or text from the original project (see `NOTICE.md`).

The source of truth is a private Azure DevOps repo; GitHub is a read-only mirror that only fast-forwards. Never assume a
direct push to GitHub `main`.

## Layout

```
bin/nastro/            CLI + TUI entry point (clap); subcommands: record, records, transcribe, config
bin/nastro-tap/        capture helper, spawned by nastro, talks over stdio; pipes PCM to an ffmpeg subprocess
crates/nastro-config/  TOML config, defaults, and `edit.rs` (in-place `config set`, via toml_edit)
crates/nastro-records/ filesystem-as-database scan/formatting
crates/nastro-record/  recording session orchestration
crates/nastro-audio/   capture/mixer core + WASAPI / PulseAudio backends
crates/nastro-tui/     ratatui screens + state machine
crates/nastro-transcribe/  whisper.cpp engine, model download/verify, WhisperX driver (`whisperx.rs`), `hf.rs`
installer/nastro-rs.iss   Inno Setup script for the Windows installer
eng/                   build, packaging, signing, release and test scripts (PowerShell)
```

## Build and check

Building needs CMake, a C/C++ compiler and **libclang** (whisper.cpp bindings). On Windows set `LIBCLANG_PATH` to a folder
containing `libclang.dll` (for example `winget install LLVM.LLVM` then `C:\Program Files\LLVM\bin`). On Linux also install
`libpulse-dev`. FFmpeg is a runtime dependency, not bundled.

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI runs these on Windows and Linux plus `cargo deny`. A new dependency must be MIT/Apache-2.0/BSD-compatible, and
`THIRD_PARTY_NOTICES.md` is generated (`eng/generate-notices.ps1`), never edited by hand. Behaviour changes need tests and
a `CHANGELOG.md` entry under `## [Unreleased]`.

## Rules that are easy to break

- **Secrets:** a Hugging Face token must never appear on a command line, in logs, in error text, or in test output. It is
  read from stdin or `--from-file`, and handed to WhisperX only through its environment. Keep errors free of values that may
  be credentials. Existing tests assert this; keep them passing and extend them.
- **Config edits** go through `nastro_config::set_value` so comments and other settings survive. Do not rewrite the file.
- **Installer (`installer/nastro-rs.iss`):**
  - `AppId` must never change; it is how upgrades and uninstall find the install.
  - A plain `/VERYSILENT` install must never use the network. Optional tasks run silently only when named in `/TASKS=`.
  - A failed optional step must not fail the install; it becomes a note on the last page.
  - A line in `[Code]` that starts with `#` is read as a preprocessor directive. Keep `#13#10` mid-line.
  - The PATH edit is exact: append once, keep REG_EXPAND_SZ, remove only that entry on uninstall.
  - Test with `eng/test-installer-deps.ps1` (offline, stand-in winget and whisperx), which the release pipeline also runs.
- **Signing:** `eng/sign-file.ps1` is the Inno sign hook. It signs a `.exe` copy of Inno's `.tmp` files (the signing CLI
  picks by extension) and runs from the repo root, because `.az-sign.json` is read from the current directory. Releases are
  signed by Azure Artifact Signing; never commit a secret or certificate.
- **Releases:** the release pipeline is `nastro-rs-release` (`azure-pipelines-release.yml`). Do a `githubRelease=dry-run`
  first. Cut a release by bumping `[workspace.package] version` in `Cargo.toml`, moving the Unreleased changelog entries under
  the new version, committing, and pushing a signed `v<version>` tag. The pipeline fails if the tag and `Cargo.toml` differ.
  Only release when asked.
- **Pinned toolchain:** signing tools and Inno Setup versions are pinned in the pipeline on purpose.

## Conventions

- Commits: `git commit -s` (DCO sign-off is required), SSH-signed, concise message in the style of `git log`.
- Windows scripts are PowerShell 7. When writing text that contains backticks (markdown) from PowerShell, use single-quoted
  here-strings (`@'...'@`); in double quotes the backtick is an escape and corrupts the text.
- Line endings: the working tree may be CRLF while git normalises; do not churn whole files for line endings.
- Platform-specific code (WASAPI, PulseAudio, console signals) is hard to test in CI; say how and where you verified it.
- Never put recordings, transcripts or other private data into issues, tests or fixtures.

# Changelog

All notable changes to this project are documented in this file. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Cargo workspace scaffold: `nastro-config`, `nastro-records`, `nastro-record`,
  `nastro-audio`, `nastro-tui` library crates, plus `nastro` (CLI/TUI) and
  `nastro-tap` (capture helper) binaries.
- `nastro-config`: TOML config loading/overlay onto defaults, `~`-expansion,
  `HF_TOKEN` env fallback, max-parallel floor - ported 1:1 from the original Go
  `internal/config` package, adapted for cross-platform (Windows/Linux) paths via the
  `directories` crate.
- `nastro-records`: filesystem-as-database scan, `YYYY-MM-DD-HHMM[-slug]` directory
  name parse/build, metadata.json duration parsing, human-readable duration/size
  formatting, tab-separated `--plain` output, transcript preview truncation - ported
  1:1 from `internal/records`.
- `nastro records`/`nastro records --plain` CLI commands are fully functional against
  real recording directories. `nastro record`/`nastro transcribe`/the bare TUI are
  scaffolded stubs pending later phases (audio capture, transcription, TUI).
- License-compliance CI gate (`cargo-deny`: licenses/advisories/bans/sources) and
  generated `THIRD_PARTY_NOTICES.md` (`cargo-about`).
- `NOTICE.md` crediting the original `scaccogatto/nastro` (MIT) as the design/behavior
  reference this port is built from.
- Azure Pipelines CI (build/test/clippy/fmt/license-compliance) on a Windows/Linux
  matrix.

### Scope notes

- Target platforms: Windows + Linux only (no macOS) - see repo-level design notes.
- Audio format: AAC/.m4a via an `ffmpeg` subprocess (incremental, fragmented-MP4 for
  crash safety) - planned for the capture phase, not yet implemented.
- Transcription: planned via `whisper-rs` (embedded whisper.cpp) in a later phase;
  diarization (whisperx/pyannote) deferred/likely remains an optional external-process
  integration.

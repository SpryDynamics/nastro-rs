# Changelog

All notable changes to this project are documented in this file. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added (Phase 2 - record session orchestration)

- `nastro-record`: fleshed out from a stub into a functional crate, ported from the
  Go `internal/record` package:
  - `slugify`: ASCII-fold + lowercase + dash-collapse recording-name slugs.
  - `acquire_lock`/`LockHandle`: the single global `.lock` file lifecycle (atomic
    create, PID content, stale-lock recovery via an injectable `is_process_alive`,
    corrupt-content recovery, RAII release on drop).
  - `is_process_alive`: cross-platform PID-liveness check via `sysinfo` (replaces
    the original's POSIX-only `syscall.Kill(pid, 0)`).
  - `parse_level_line`: the `level sys=.. mic=..` stdout contract parser.
  - `describe_tap_exit`: the 0 (clean) / 2 (permission denied) / other (failure)
    exit-code contract.
  - `should_discard`: the &lt;2s "accidental start/stop" discard rule.
  - `check_low_disk_space`/`disk_space_warning`: the 1 GiB low-disk warning
    (`fs4` for the real cross-platform free-space query).
  - `prevent_sleep`/`SleepGuard`: anti-sleep for the duration of a recording -
    `SetThreadExecutionState` on Windows, best-effort `systemd-inhibit` on Linux
    (replaces `caffeinate`).
  - `request_graceful_stop`: `SIGINT` (Unix) / `GenerateConsoleCtrlEvent` +
    `CREATE_NEW_PROCESS_GROUP` (Windows) - replaces the original's Unix-only
    signal handling.
  - `resolve_tap_path`/`TapProcess`: spawns `nastro-tap`, streams parsed level
    readings over a channel, captures a stderr tail, and exposes non-blocking
    (`try_exit`) and bounded-wait-then-force-kill (`wait_with_timeout`) exit
    handling.
  - 38 unit tests, including an IPC-protocol smoke test that spawns a trivial
    shell one-liner standing in for the not-yet-implemented real `nastro-tap`
    binary, validating stdout/stderr/exit-code plumbing end-to-end.
- `nastro record` is now fully wired to `nastro-record`: disk-space check, lock
  acquire/release, dated+slugged recording directory creation, `nastro-tap`
  resolution/spawn, Ctrl+C → graceful-stop → bounded force-kill, and a
  non-zero exit code on capture failure. Still can't actually capture audio
  (that's `nastro-audio`, a later phase) - `nastro-tap` itself remains a stub that
  reports "not yet implemented" and exits 1, which `nastro record` now surfaces
  correctly end-to-end.

### Added (Phase 1 - OS-agnostic core)

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

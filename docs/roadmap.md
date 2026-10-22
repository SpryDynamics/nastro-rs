# Roadmap

Phased build plan (see `CHANGELOG.md` for what's actually landed so far).

## Phase 1 - OS-agnostic core (done)
- `nastro-config`, `nastro-records`: config loading, filesystem-as-database scan,
  formatting helpers. `nastro records`/`records --plain` fully functional.
- Workspace scaffold, CI (build/test/clippy/fmt matrix on Windows+Linux),
  license-compliance gate (`cargo-deny`), generated `THIRD_PARTY_NOTICES.md`.

## Phase 2 - Record session orchestration (`nastro-record`)
- Lockfile lifecycle (atomic create, stale-PID detection via `sysinfo`).
- Disk-space check (`fs4`/platform APIs).
- Anti-sleep: `SetThreadExecutionState` (Windows) / best-effort `systemd-inhibit`
  (Linux).
- Spawn `nastro-tap`, parse `level sys=.. mic=..` stdout lines, exit-code contract
  (0 = clean stop, 2 = permission denied, 1 = other failure).
- Graceful stop: Windows console control events (`CTRL_BREAK_EVENT` +
  `CREATE_NEW_PROCESS_GROUP`) vs. Unix `SIGINT`/`SIGKILL`.

## Phase 3 - Audio capture (`nastro-audio` + `nastro-tap`)
- Platform-agnostic ring buffer / wall-clock-driven mixer / rate-drift watcher
  (ported from the original Swift `SampleMixer`/`AudioRingBuffer`/`RateWatcher`
  design - see the original project's `docs/stack.md` for the sample-rate-mismatch
  postmortem that shaped that design).
- Windows: `wasapi` crate, loopback capture on the default render endpoint + normal
  mic capture, device-change handling via `IMMNotificationClient`.
- Linux: mic via `cpal` (ALSA); system audio via `libpulse-binding` opening
  `<default-sink>.monitor` (works under PulseAudio and PipeWire's pulse-compat
  layer), default-sink-change handling via Pulse's subscribe API.

## Phase 4 - Incremental AAC/M4A muxing
- `ffmpeg` subprocess, PCM piped via stdin, fragmented-MP4 flags
  (`+frag_keyframe+empty_moov+default_base_moof`) for crash safety.
- `metadata.json` duration writer (every 5s, matching the original cadence).

## Phase 5 - TUI/CLI (`nastro-tui`, `bin/nastro`)
- `ratatui` + `crossterm` port of list/name-form/recording/detail/help screens.
- Reveal-in-folder (`explorer /select,`/`xdg-open`), delete→Trash (`trash` crate),
  OSC-8 hyperlinks (unchanged, terminal-level feature).

## Phase 6 - Packaging/CI/release
- Release artifacts (Windows zip with both binaries side by side; Linux tarball).
- Windows binaries signed via `az-artifact-sign` (separate tool,
  `dev.azure.com/sprydynamics/oss-tools/_git/az-artifact-sign`).

## Phase 7 - Transcription (deferred)
- `nastro-transcribe` crate wrapping `whisper-rs` (vendored whisper.cpp), ggml model
  management (download + SHA-256 verify), resampling via the Phase 4 ffmpeg
  dependency.
- Diarization (whisperx/pyannote): no clean embeddable Rust equivalent identified;
  likely stays an optional external-process integration, revisited after core
  transcription ships.

## Explicitly out of scope
- macOS support (the original project's primary/only target) - this port targets
  Windows + Linux only.
- NTFS ACL tightening beyond default per-user-profile privacy (POSIX 0600/0700 mode
  bits are applied on Linux; Windows has no equivalent construct - see
  `nastro_config::mode`).

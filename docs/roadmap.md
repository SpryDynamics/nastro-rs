# Roadmap

Phased build plan (see `CHANGELOG.md` for what's actually landed so far).

## Phase 1 - OS-agnostic core (done)
- `nastro-config`, `nastro-records`: config loading, filesystem-as-database scan,
  formatting helpers. `nastro records`/`records --plain` fully functional.
- Workspace scaffold, CI (build/test/clippy/fmt matrix on Windows+Linux),
  license-compliance gate (`cargo-deny`), generated `THIRD_PARTY_NOTICES.md`.

## Phase 2 - Record session orchestration (`nastro-record`) (done)
- Lockfile lifecycle (atomic create, stale-PID detection via `sysinfo`).
- Disk-space check (`fs4`/platform APIs).
- Anti-sleep: `SetThreadExecutionState` (Windows) / best-effort `systemd-inhibit`
  (Linux).
- Spawn `nastro-tap`, parse `level sys=.. mic=..` stdout lines, exit-code contract
  (0 = clean stop, 2 = permission denied, 1 = other failure).
- Graceful stop: Windows console control events (`CTRL_BREAK_EVENT` +
  `CREATE_NEW_PROCESS_GROUP`) vs. Unix `SIGINT`/`SIGKILL`.
- `nastro record` CLI wired end-to-end against the stub `nastro-tap` (disk check,
  lock, dir creation, spawn, Ctrl+C → graceful stop → force-kill, exit code).
- Not yet exercised against *real* capture, since `nastro-audio`/`nastro-tap`
  (Phase 3) don't actually capture audio yet.

## Phase 3 - Audio capture (`nastro-audio` + `nastro-tap`) (done, Windows-verified)
- Platform-agnostic ring buffer / wall-clock-driven mixer / rate-drift watcher
  (ported from the original Swift `SampleMixer`/`AudioRingBuffer`/`RateWatcher`
  design - see the original project's `docs/stack.md` for the sample-rate-mismatch
  postmortem that shaped that design). 29 unit tests.
- Windows: `wasapi` crate, loopback capture on the default render endpoint + normal
  mic capture, both shared-mode with `autoconvert`. **Verified against real
  hardware**: both sources captured real, correctly-rated audio.
- Linux: system audio via `libpulse-binding`/`libpulse-simple-binding` opening
  `<default-sink>.monitor` (device name resolved via `pactl get-default-sink`;
  works under both PulseAudio and PipeWire's pulse-compat layer); mic via the
  same blocking `Simple` API against the default source. **Compiles under CI's
  Linux leg only - not validated against real hardware** (no Linux audio stack
  available in this environment). `cpal` was considered for mic capture but
  dropped in favor of reusing the same Pulse `Simple` API for both sources,
  avoiding a second audio stack dependency.
- Not yet implemented: `IMMNotificationClient`/Pulse-subscribe-API device-change
  handling (reacting to the default device changing mid-recording) - deferred,
  not blocking for a first working capture path.
- `nastro-tap` wired to real capture end-to-end (initially with a throwaway WAV
  writer, replaced by the Phase 4 AAC/M4A encoder).

## Phase 4 - Incremental AAC/M4A muxing (done)
- `ffmpeg` subprocess, PCM piped via stdin, fragmented-MP4 flags
  (`+frag_keyframe+empty_moov+default_base_moof`, `-frag_duration 1s`,
  `-flush_packets 1`) for crash safety. Replaced the interim WAV writer.
- `metadata.json` duration writer (every 5s, matching the original cadence).
- Not yet done: `nastro transcribe`'s 16 kHz resample step (Phase 7) will reuse this
  same ffmpeg dependency.

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

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
- Linux: system audio via `libpulse-binding`/`libpulse-simple-binding` opening the
  server-resolved `@DEFAULT_MONITOR@` (falling back to `pactl get-default-sink` +
  `.monitor`); mic via the same blocking `Simple` API against the default source.
  Both are meant to work under PulseAudio and PipeWire's pulse-compat layer.
  **Verified (Phase 6) against a real PulseAudio-protocol server - WSLg on Ubuntu
  24.04 - for opening, reading, encoding and stopping.** Only silence was observed
  there, so audio content, and a native PipeWire/PulseAudio desktop, remain
  unverified. `cpal` was considered for mic capture but dropped in favor of reusing
  the same Pulse `Simple` API for both sources, avoiding a second audio stack.
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
- The 16 kHz resample step for `nastro transcribe` (Phase 7) reuses this same ffmpeg dependency.

## Phase 5 - TUI/CLI (`nastro-tui`, `bin/nastro`) (done, Windows-verified)
- `ratatui` port of the list / name-form / recording / detail / help screens, with
  filter, confirm modals, and live timer + dB level meters.
- Delete → Recycle Bin / Trash (`trash` crate; never falls back to permanent delete),
  reveal in file manager (`explorer /select,` / `xdg-open`).
- Shared `RecordingSession` in `nastro-record`, used by both the TUI and headless CLI.
- Deliberately omitted: OSC 8 clickable paths. ratatui's cell buffer counts every
  character of an escape sequence as visible, which corrupts layout.
- Not yet exercised live: reveal-in-Explorer (opens a window) and the Linux TUI path
  against a real Linux terminal/trash.
- The `t` key / `nastro transcribe` are implemented in Phase 7.

## Phase 6 - Packaging/CI/release (done; see `docs/releasing.md`)
- Release artifacts: Windows zip and Linux tarball, each with both binaries, the
  license files and a `REQUIREMENTS.txt`, plus `SHA256SUMS`. Built `--locked`.
- Windows binaries signed (and RFC 3161 timestamped) via `az-artifact-sign` using a
  secretless, workload-identity-federated service connection whose only Azure
  permission is Signer on the single certificate profile; branch-restricted.
- The shipped archives are verified, not just built: signatures checked on the
  extracted binaries, the Linux tarball smoke-tested (`ldd`, checksum), and both
  downloaded and run on real machines.
- One-way GitHub mirror (SpryDynamics/nastro-rs) via a repo-scoped deploy key.
- GitHub releases: draft -> verify (assets downloaded back and compared) -> publish, never from an unsigned build.
- GitHub issues/PRs synced two-way with Azure Boards (private); PRs are ported by hand to keep the mirror fast-forward only.
- Not done: aarch64, installers (MSI/winget/deb).

## Phase 7 - Transcription (done; Windows- and Linux-verified)
- `nastro-transcribe`: `ffmpeg` decodes `audio.m4a` to 16 kHz mono f32, embedded whisper.cpp (`whisper-rs`, statically
  linked) recognises it, `transcript.txt` + `transcript.srt` are written atomically with private permissions.
- Models: pinned `ggerganov/whisper.cpp` revision, size + SHA-256 verified before use, downloaded only with consent.
- CLI `nastro transcribe <id|last>` (`--model`, `--lang`, `--threads`, `--force`, `--yes`, `--list-models`) and the TUI's
  background jobs (progress, queue, cancel, confirmations).
- Verified with real speech and a real model on Windows and on Linux (Ubuntu 24.04 under WSL2), including through the
  real TUI in a pseudo-terminal; default model `large-v3-turbo` measured at ~2x real time on a 16-core CPU.
- Speaker labels: the original's `whisperx` backend, as an optional external tool (`transcriber = "whisperx"`): installed with
  `uv` on request, run as a subprocess, parsed into `[SPEAKER_00]`-labelled `txt`/`srt`; token via environment only; cancel kills
  the whole process tree. Verified end to end against the real tool, including the speaker step: a synthetic two-voice, eight-turn conversation (53 s, `small` model, Windows) came back with every turn attributed to the right speaker.
- Not done / next:
  - GPU acceleration (Vulkan, which would also cover AMD/Intel): needs a decision on how to ship a binary that still
    starts on machines without a Vulkan loader.
  - An embedded (no Python, no account) replacement for WhisperX's speaker identification, e.g. ONNX diarization models.
  - Long recordings are decoded whole into memory (limit 6 hours); chunked streaming would lift that.
  - aarch64 builds (the AVX2 baseline is x86-64 only).
## Explicitly out of scope
- macOS support (the original project's primary/only target) - this port targets
  Windows + Linux only.
- NTFS ACL tightening beyond default per-user-profile privacy (POSIX 0600/0700 mode
  bits are applied on Linux; Windows has no equivalent construct - see
  `nastro_config::mode`).

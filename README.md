# nastro-rs

A Rust port of [scaccogatto/nastro](https://github.com/scaccogatto/nastro) - a
terminal-native call recorder (system audio + mic, local files, on-device
transcription) - retargeted from **macOS-only** to **Windows + Linux**.

> **Status: core recording pipeline works on Windows.** Config loading, the
> recordings browser, session orchestration, and **real audio capture** are all
> functional: `nastro record` actually records system audio + mic to a playable
> file on Windows today (WASAPI, verified against real hardware). The Linux
> capture backend compiles but is unvalidated (no Linux audio stack available in
> this dev environment). The audio format is an interim 16-bit WAV, not yet the
> final AAC/.m4a (that's Phase 4). Transcription and the TUI are not yet
> implemented. See [`CHANGELOG.md`](CHANGELOG.md) for exact scope and
> [`docs/`](docs/) for the phased build plan.

## Why a port, and why these platform/format choices

The original project is excellent but deliberately macOS-only: its audio capture
leans on CoreAudio's process-tap API (Swift), and several supporting pieces
(`afconvert`, `caffeinate`, Homebrew-based installers, `~/.Trash`, POSIX file modes)
are macOS-specific. This port keeps the overall architecture - two cooperating
processes (`nastro` the CLI/TUI, `nastro-tap` the capture helper) talking over
stdio, filesystem-as-database record storage, crash-safe incremental writes - while
replacing every macOS-specific piece with a Windows/Linux-native equivalent:

| Concern | Original (macOS) | This port |
| --- | --- | --- |
| System audio capture | CoreAudio process tap | WASAPI loopback (Windows) / PulseAudio monitor source (Linux) |
| Audio container | AVAudioFile → AAC/.m4a | `ffmpeg` subprocess, fragmented MP4 for crash safety |
| Resample for transcription | `afconvert` | `ffmpeg` subprocess (reuses the muxing dependency) |
| Transcription | shells out to Homebrew-installed `whisper-cli`/`whisperx` | embedded `whisper-rs` (whisper.cpp) - no external installer |
| Anti-sleep | `caffeinate` | `SetThreadExecutionState` (Windows) / best-effort `systemd-inhibit` (Linux) |
| Trash on delete | `~/.Trash` | `trash` crate (Recycle Bin / freedesktop trash spec) |
| TUI | Bubble Tea (Go) | `ratatui` |

See `NOTICE.md` for attribution to the original project.

## Workspace layout

```
crates/
  nastro-config/   TOML config load/defaults (done)
  nastro-records/  filesystem-as-database scan/formatting (done)
  nastro-record/   recording session orchestration (done)
  nastro-audio/    capture/mixer core + platform backends (done; Windows-verified, Linux compile-only)
  nastro-tui/      ratatui screens (scaffold)
bin/
  nastro/          CLI/TUI entry point
  nastro-tap/      capture helper (spawned by nastro, talks over stdio)
```

## Build

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
./eng/check-licenses.ps1   # cargo-deny: licenses/advisories/bans/sources
```

## Try it

```powershell
cargo run --bin nastro -- records --plain
cargo run --bin nastro -- records
cargo run --bin nastro -- record --name "Weekly Sync"   # records real system audio + mic to a .wav (Windows)
```

Config is read from `%APPDATA%\nastro\config.toml` (Windows) /
`~/.config/nastro/config.toml` (Linux); see `nastro-config`'s `ConfigFile` for the
available keys. A missing config file is not an error - sane defaults apply
(`~/Recordings/nastro`, `large-v3-turbo`, etc.).

## License

MIT, see [`LICENSE`](LICENSE). This is an independent reimplementation (see
[`NOTICE.md`](NOTICE.md)) - no source from the original project is copied.
Third-party Rust dependency licenses are tracked in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) and gated in CI via `cargo-deny`
(`deny.toml`).

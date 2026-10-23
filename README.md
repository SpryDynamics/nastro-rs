# nastro-rs

A Rust port of [scaccogatto/nastro](https://github.com/scaccogatto/nastro) - a
terminal-native call recorder (system audio + mic, local files, on-device
transcription) - retargeted from **macOS-only** to **Windows + Linux**.

> **Status: recording and browsing work on Windows; transcription is not built
> yet.** Run `nastro` for a full-screen terminal UI (browse, record, filter,
> delete to the Recycle Bin, reveal in Explorer), or `nastro record` for headless
> recording. Capture is verified against real hardware on Windows (WASAPI). The
> Linux capture backend compiles and its UI is tested, but it has never run
> against a real Linux audio stack (none is available in this dev environment).
> Recordings are AAC/.m4a (fragmented MP4, so they survive a crash) via an
> external `ffmpeg`. `nastro transcribe` and the `t` key are placeholders. See
> [`CHANGELOG.md`](CHANGELOG.md) for exact scope and [`docs/`](docs/) for the
> phased build plan.

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
  nastro-tui/      ratatui screens + state machine (done)
bin/
  nastro/          CLI/TUI entry point
  nastro-tap/      capture helper (spawned by nastro, talks over stdio)
```

## Requirements

- **FFmpeg** on `PATH` (or set `NASTRO_FFMPEG` to the executable). It is run as a
  separate process to encode AAC/.m4a and is not bundled - see
  [`NOTICE.md`](NOTICE.md). Windows: `winget install Gyan.FFmpeg`;
  Debian/Ubuntu: `sudo apt install ffmpeg`.

## Build

**Linux only**: install the PulseAudio dev headers/libs first (needed to link
`nastro-audio`'s system-audio/mic backend):
```bash
sudo apt-get install -y libpulse-dev   # Debian/Ubuntu
# or: sudo dnf install -y pulseaudio-libs-devel   # Fedora
```

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
./eng/check-licenses.ps1   # cargo-deny: licenses/advisories/bans/sources
```

## Try it

```powershell
cargo run --bin nastro                                   # the TUI
cargo run --bin nastro -- records --plain                # scriptable, tab-separated
cargo run --bin nastro -- record --name "Weekly Sync"    # headless; Ctrl+C stops & saves
```

When stdin/stdout isn't a terminal (piped, redirected), `nastro` and
`nastro records` print the plain table instead of opening the TUI.

### TUI keys

| Key | Action |
| --- | --- |
| `r` | record (name + capture mode; `Tab` cycles system+mic / mic only / system only) |
| `s` / `q` / `Ctrl+C` | stop and save (while recording; under 2 seconds is discarded) |
| `x` | discard the live recording (asks first) |
| `Enter` | open a recording (shows a transcript preview if one exists) |
| `o` | reveal in Explorer / file manager |
| `d` | delete to the Recycle Bin / Trash (asks first; only `y` confirms) |
| `/` | filter by name |
| `j` `k` / arrows, `g` `G` | move, first, last |
| `?` | help |
| `q` / `Esc` | back / quit |

Delete never falls back to a permanent delete: if the Recycle Bin / Trash can't be
used you get an error and nothing is removed.

### Config

`--config <path>` (or the `NASTRO_CONFIG` environment variable) selects a config
file explicitly; a file you name that fails to parse is an error rather than a
silent fall back to defaults. Otherwise config is read from `%APPDATA%\nastro\config.toml` (Windows) /
`~/.config/nastro/config.toml` (Linux); see `nastro-config`'s `ConfigFile` for the
available keys. A missing config file is not an error - sane defaults apply
(`~/Recordings/nastro`, `large-v3-turbo`, etc.).

## License

MIT, see [`LICENSE`](LICENSE). This is an independent reimplementation (see
[`NOTICE.md`](NOTICE.md)) - no source from the original project is copied.
Third-party Rust dependency licenses are tracked in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) and gated in CI via `cargo-deny`
(`deny.toml`).

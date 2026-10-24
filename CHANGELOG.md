# Changelog

All notable changes to this project are documented in this file. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- **TUI colors**: a cyan accent for the title, the selected row (now marked with `▸` and bold instead of inverted video),
  panel titles, level meters, speaker labels and key names; dim gray for borders, labels and hints; red for recording and
  errors. Only the terminal's 16 ANSI colors are used, never fixed RGB, so it follows the user's terminal theme. `NO_COLOR`
  (and `TERM=dumb`) turn colors off entirely.
- Transcript preview shows one line per speaker turn, with `[SPEAKER_00]` labels in the accent color. (Newlines were being
  dropped, gluing consecutive turns together.)

### Fixed

- The help screen cut off two of its lines (`...asks before do`, `...transcriptio`) since transcription was added.
- Dialogs whose question wraps to a second line (the model download prompt) lost their own `y yes / any other key cancel`
  row off the bottom of the box; the box now grows with its text.

### Added

- README header: logo, tagline, badges and an **animated demo of the real TUI** (`docs/assets/demo.svg`), produced by
  `eng/render-demo.ps1`: it runs the actual binary in a pseudo-terminal on sample recordings and records the screens, so it
  cannot drift from what the program does. The path shown in the picture is redacted.
## [0.1.0] - 2026-10-24

First release (pre-release: the project is 0.x). Recording, browsing and on-device transcription on Windows and Linux,
plus optional speaker labels.

### Added (Phase 7 - transcription)

- **`nastro transcribe <id|last>`** and the TUI's **`t`** key: on-device speech-to-text, written as `transcript.txt`
  (one phrase per line) and `transcript.srt` next to `audio.m4a`. New crate `nastro-transcribe`: `ffmpeg` decodes the
  recording to 16 kHz mono, an embedded **whisper.cpp** (via `whisper-rs`, statically linked) recognises it.
  Language is configurable or auto-detected. Works offline after the model download.
- **Models** (`whisper_model`, default `large-v3-turbo`; `nastro transcribe --list-models`): downloaded on first use from
  a revision of `ggerganov/whisper.cpp` pinned in the source, over HTTPS, and verified against a built-in SHA-256 and
  size *before* the file appears under its final name; a bad download is discarded and never replaces a good model.
  nastro asks before downloading (`--yes` to skip). `whisper_model` may also be the path of a ggml `.bin`.
- **TUI**: transcription runs in the background with live progress in the list and detail views, a queue honouring
  `max_parallel_transcriptions`, cancel (`c`), and confirmations before replacing a transcript, downloading a model, or
  quitting while a job runs. The footer hints adapt to the terminal width so help and quit are never cut off.
- **Safe by construction**: a transcript is replaced only once the new one is complete (a cancelled or failed run leaves
  the old one untouched); both files are private (`0600` on Unix); a recording still being recorded is refused; silence
  produces an empty transcript, not invented words; cancelling takes effect within a fraction of a second.
- `.cargo/config.toml` pins whisper.cpp to a portable x86-64 baseline (AVX2/FMA/F16C) instead of the build machine's CPU,
  and `nastro` checks for those at run time and says so rather than crashing.
- CI builds whisper.cpp on Windows and Linux (`eng/ci-native-deps.ps1`) and runs the real transcription tests - library,
  CLI and a real TUI session in a pseudo-terminal - against a downloaded `tiny.en` model; `NASTRO_REQUIRE_WHISPER=1`
  turns a skip into a failure. Public-domain speech fixture: `crates/nastro-transcribe/tests/fixtures/jfk.wav`.
- Licensing: whisper.cpp/ggml attribution added to `NOTICE.md`; `CDLA-Permissive-2.0` (the CA-list data in
  `webpki-roots`, used for the model download's TLS) added to the `cargo-deny` and `cargo-about` allow-lists.

### Added (speaker labels via WhisperX)

- **`transcriber = "whisperx"`** (or `nastro transcribe --transcriber whisperx`): speaker identification. nastro runs the
  external [WhisperX](https://github.com/m-bain/whisperX) tool and writes the same `transcript.txt` / `transcript.srt`
  with `[SPEAKER_00]` labels, merging consecutive phrases by the same speaker. `--speakers N` (or `--min-speakers` /
  `--max-speakers`) tells it how many people to expect. It is optional because it needs a Hugging Face account (the
  pyannote speaker models are gated) and about 2 GB for Python/PyTorch.
- **`nastro transcribe --setup-whisperx`** installs WhisperX with `uv` (user-level, pinned to Python 3.12) and fetches
  the NLTK `punkt_tab` data a fresh install lacks (without it WhisperX 3.8.6 crashes minutes in, at alignment). The same
  data check runs automatically before every run. Nothing is installed without being asked: the CLI offers (`--yes` to
  accept), the TUI only reports what is missing.
- The Hugging Face token reaches WhisperX only through its **environment** (never argv, which other local users can
  read), is never printed, and is scrubbed from anything echoed back in an error; `Options`'s `Debug` output redacts it.
- WhisperX has no percentage, so the CLI and TUI show its **stage** (detecting speech, transcribing, aligning words,
  identifying speakers).
- **Cancelling** (Ctrl+C / `c`) kills WhisperX *and everything it started*, on Windows with `taskkill /T` and on Unix by
  signalling the process group, and removes its temporary folder; an existing transcript is never touched.
- Errors say what to do: no WhisperX / no token / **a rejected token (HTTP 401) versus valid token without the model
  terms accepted (403)**, told apart from Hugging Face's real error text; unusable model names; missing data.
- Tests: a stand-in `whisperx` (`nastro-fake-whisperx`, never shipped) covers the whole integration in CI on both OSes;
  mutation checks confirmed the token-in-argv and process-tree guarantees are really tested. Against the **real** WhisperX
  (opt-in: `NASTRO_TEST_REAL_WHISPERX=1`) the output format, stage markers, automatic NLTK repair and that cancelling
  leaves no Python running are verified. The speaker step needs a token (`NASTRO_TEST_HF_TOKEN`, `NASTRO_TEST_DIALOGUE`);
  it was verified by hand on Windows with a two-voice, eight-turn synthetic conversation (every turn labelled correctly,
  `--speakers 2`, `small` model). Linux and real human recordings (overlap, noise, more than two voices) are not yet checked.
### Changed

- The `transcriber` config value is now `whisper` (the built-in engine). `whisper-cli` is accepted as an alias;
  `whisperx` selects speaker labels.
- `nastro` is now about 4.7 MB (was under 1 MB) because it contains the speech engine.
- Building from source needs CMake, a C++ compiler and libclang (see the README).

### Fixed (found while building the above)

- whisper-rs 0.16's `set_abort_callback_safe` registers a trampoline for the closure's concrete type but passes it a
  pointer to a boxed trait object, which is undefined behaviour for any closure that captures state (we saw a
  spurious abort, error -6). nastro uses the raw callback API with correct types instead.
- With MSVC and the default Visual Studio generator, the `cmake` crate replaces CMake's Release flags and drops `/O2`,
  so whisper.cpp was compiled without optimisation (about 10x slower, and slow to cancel). Fixed in `.cargo/config.toml`.
- Thread count: whisper.cpp threads spin-wait, so using every hardware thread makes recognition collapse (measured: 8
  threads on a 6-CPU machine took 13 minutes where 4 took 20 seconds). The default is now one per physical core (at most
  16) and any request is capped to that.
### Added (GitHub releases, issues and pull requests)

- The release pipeline now ends with a **Publish to GitHub** stage: a draft release is created, every asset is
  downloaded back and compared byte-for-byte with what was built, and only then is it published (`0.x` as a
  pre-release). It refuses to run for an unsigned build. A `githubRelease=dry-run` parameter exercises everything
  except publishing against a throwaway tag.
- GitHub Issues and pull requests are enabled and synced with the maintainers' private Azure Boards by a separate
  service. Only items that began on GitHub (or are explicitly marked public) are ever visible on GitHub, and only
  comments that start with `/public` are published. Pull requests are ported by hand, keeping the mirror
  fast-forward only.
- Issue forms, a PR template, `CONTRIBUTING.md` (DCO sign-off) and `SECURITY.md` (private vulnerability reporting).
- A ruleset on `main` blocks direct pushes, force pushes and deletion; only the mirror pipeline can update it.

### Fixed

- `eng/release-to-github.ps1` reported failure after a *successful* dry run, because the final "is the throwaway tag
  gone?" check is expected to fail (404) and left a non-zero exit code behind.
### Added (Phase 6 - release packaging and signing)

- `eng/package.ps1` packages built release binaries into
  `nastro-<ver>-<os>-<arch>.zip` / `.tar.gz` with a `sha256sum`-compatible checksum.
  Each archive carries `LICENSE`, `NOTICE.md`, `THIRD_PARTY_NOTICES.md`, `README.md`,
  `CHANGELOG.md` and a `REQUIREMENTS.txt`; the script fails if any is missing.
- `azure-pipelines-release.yml` (`nastro-rs-release`): runs on `v*` tags or by hand,
  builds with `--locked`, requires the tag to equal the `Cargo.toml` version, signs the
  Windows binaries, unpacks the *shipped* zip and runs `signtool verify` on what's in
  it, smoke-tests the Linux tarball (runs it, `ldd`, checksum), and writes `SHA256SUMS`.
- Windows signing uses `az-artifact-sign` 0.1.0 (pinned, as is the `sign` CLI it drives)
  through a new service connection `nastro-artifact-signing`. It uses workload identity
  federation, so **there is no secret**; the Entra app has no password or certificate
  credentials, one federated credential bound to that connection, and a single Azure role
  - *Artifact Signing Certificate Profile Signer* on the one certificate profile. Only the
  release pipeline may use the connection and a branch-control check limits it to `main` and
  `v*` tags. Details and rebuild steps are in `docs/releasing.md`.
- Verified on the published artifacts of a real pipeline run: checksums match; the
  extracted Windows binaries verify with `signtool` (issued to Spry Dynamics. LLC,
  timestamped) and run; the Linux tarball runs on a real Ubuntu 24.04 with executable bits
  and libpulse resolved.

### Fixed (found by running the packaged Linux artifact on real Ubuntu)

- **SIGTERM/SIGHUP killed a recording ungracefully.** `ctrlc` only handled SIGINT, so
  `kill`, `timeout`, a service manager, or closing the terminal window left a stale lock and
  an unfinalized recording. Enabled `ctrlc`'s `termination` feature (also CTRL_CLOSE /
  LOGOFF / SHUTDOWN on Windows); the TUI now saves a live recording before exiting. Re-run
  on the CI-built artifact: stopped by SIGTERM after 8s, it printed `Saved`, released the
  lock, left no orphan process, and wrote a valid 8.0s AAC file with matching metadata.
- **System-audio capture on Linux needed `pactl`** only to look up the default sink, and
  failed with an unhelpful "no capture device" without it. It now asks the server for
  `@DEFAULT_MONITOR@` (PulseAudio and PipeWire-pulse resolve it); `pactl` is only a
  fallback. Re-run on the CI-built artifact with no `pactl` installed: system capture
  started, reported levels and produced a file.
- **`nastro-tap` panicked on a broken pipe** when its parent vanished (`println!`). It now
  stops cleanly so ffmpeg can finalize, instead of recording on with nobody to stop it.
- **An explicit `--config` / `NASTRO_CONFIG` pointing at a missing file silently fell back
  to defaults** (my Phase 5 guard only covered files that failed to *parse*), so a typo
  recorded into a directory you never chose. It is now an error. The first real symptom was
  a test recording landing in the WSL home directory. New `bin/nastro/tests/cli.rs` covers
  it, and a negative control confirmed the tests fail without the guard.
- `REQUIREMENTS.txt` formatting; `pactl` documented as optional.

### Notes

- The Linux capture backend had never actually run before this phase. WSLg's
  PulseAudio server let it open, read, encode and stop for both mic and system sources.
  Only silence was observed (nothing was playing, no mic signal), so audio *content*
  and a native PipeWire/PulseAudio desktop remain unverified.
- The signature check on the Linux side of CI is `ldd`-and-run only; there is no
  equivalent signing for Linux archives (checksums only).
- New unix-only pty test: SIGTERM and SIGHUP must exit 0 and restore the terminal. It could
  not be run on this Windows machine; it is exercised by the Linux CI leg.

### Added (Phase 5 - terminal UI)

- `nastro` (no args) and `nastro records` open a full-screen `ratatui` UI when attached
  to a terminal, and print the plain table when piped. Screens: list (with `/` filter),
  name form (capture-mode picker), live recording (timer + dB-scaled level meters),
  detail (transcript preview), help, and `y`-to-confirm delete/discard modals.
- Architecture: `nastro-tui::app` is a pure state machine (key in, `Effect`s out, no
  I/O), `view` renders it, `actions` does the OS work, `lib.rs` is the event loop. 58
  unit tests, including render tests that assert on actual screen content via
  ratatui's `TestBackend`.
- Delete moves a recording to the Recycle Bin / Trash and was verified to land in this
  machine's real Recycle Bin. It deliberately does **not** fall back to a permanent
  delete when the trash is unavailable (the original macOS build did); you get an error
  and nothing is removed.
- New `RecordingSession` in `nastro-record` (lock, directory, anti-sleep, supervised
  `nastro-tap`, bounded stop), shared by the TUI and the headless CLI. 13 new tests.
- End-to-end tests run the real `nastro` in a real pseudo-terminal (ConPTY on Windows)
  with real keystrokes and read the screen back through a terminal emulator
  (`bin/nastro/tests/tui_pty.rs`). Three run by default; three `#[ignore]`d live tests
  need a microphone, ffmpeg and a built `nastro-tap` and cover recording through the TUI,
  discarding a live recording, and headless `nastro record` stopped by Ctrl+C. (A
  fourth ignored test, in `nastro-tui`, really moves a directory to the Recycle Bin.)
  A negative control confirmed the "terminal restored on exit" assertion does fail when
  `restore()` is removed.
- `--config <path>` / `NASTRO_CONFIG` selects a config file explicitly. A file you name
  that fails to parse is a hard error rather than a silent fall back to defaults.
- Recordings started in the same minute with the same name no longer share a directory
  (second gets a `-2` suffix) - previously the second would overwrite the first's audio.

### Fixed

- Headless `nastro record` never held the anti-sleep guard (Phase 2 documented it but
  `run_record` didn't call it) and never applied the under-2-seconds discard rule. Both
  now come from the shared session.
- A failed `nastro record` left a stale `.lock` behind: `process::exit` skips
  destructors, so the lock guard never dropped. Found by running the failure path by
  hand; the session is now dropped before exiting.
- Declared MSRV was wrong: `rust-version` said 1.82 but `wasapi`, `indexmap` and `clap`
  already need 1.85, and `ratatui` 0.30 needs 1.88. Now 1.88.

### Notes

- The Windows-only ConPTY quirk worth knowing: it sends a cursor-position query
  (`ESC[6n`) at startup and withholds *all* output until the host terminal answers. The
  test harness answers it like a real terminal; nastro itself needed no change.

### Changed (Phase 4 - AAC/.m4a encoding via ffmpeg)

- `nastro-tap` now encodes to real **AAC in fragmented MP4** by piping mixed 16-bit
  mono PCM to an `ffmpeg` subprocess (`bin/nastro-tap/src/encoder.rs`), replacing the
  interim WAV writer (removed). This also fixes a latent mismatch: `nastro record`
  has always asked for `audio.m4a`, but the interim writer was putting WAV bytes in
  that file.
- Crash safety comes from the container, not header-rewriting tricks:
  `empty_moov` + `frag_keyframe` + `frag_duration 1s`, and `-flush_packets 1`.
  Prototyping showed that without `-flush_packets` ffmpeg holds everything in its
  output buffer until close (only a 28-byte `ftyp` hit the disk after 5s), which
  would have defeated the fragmenting - so that flag is required, and is covered by
  a unit test.
- Verified against real hardware on Windows: a hard-killed `nastro-tap` (simulated
  crash) after ~7s left a valid AAC / 48 kHz / mono file of 7.0s that decodes with no
  errors, and no orphaned ffmpeg process. A graceful stop through the real
  `TapProcess`/`request_graceful_stop` path produced a clean exit with decoded
  duration matching the recorded duration (live test `crates/nastro-record/tests/
  live_tap.rs`, `#[ignore]`d - needs audio hardware + ffmpeg).
- ffmpeg is spawned in its own process group so the graceful-stop signal aimed at
  `nastro-tap` doesn't reach it independently. A negative-control run showed ffmpeg
  also finalizes cleanly if it *does* receive CTRL_BREAK on Windows, so this is
  precautionary hygiene, not a fix for an observed failure.
- ffmpeg is a runtime dependency (override path with `NASTRO_FFMPEG`); a missing
  ffmpeg produces an actionable install message and exit code 1. It is invoked as a
  separate process and not bundled - see `NOTICE.md` for the license position.
- 10 new unit tests (pure arg-building and PCM conversion, plus a real-ffmpeg
  round trip that skips itself where ffmpeg is absent). CI now installs ffmpeg on
  both legs so that round trip actually runs.

### Added (Phase 3 - audio capture, `nastro-audio` + `nastro-tap`)

- `nastro-audio` pure mixer core, ported from the original Swift
  `SampleMixer`/`AudioRingBuffer`/`RateWatcher` design: `AudioRingBuffer` (bounded,
  drops oldest on overflow), `SampleMixer` (wall-clock "frames owed" scheduling
  with fractional carry-forward, zero-fill for an inactive source in
  mic-only/system-only mode, soft-clamped sum), `RateWatcher` (2%-drift-for-two-
  consecutive-windows detection, snap to nearest standard rate - a defensive
  safety net, largely moot in practice given WASAPI's shared-mode autoconvert),
  `compute_level` (RMS for the `level sys=.. mic=..` stdout contract). 29 unit
  tests.
- **Real, working Windows audio capture**: WASAPI backend using the documented
  "initialize a Render-direction device's AudioClient for Direction::Capture"
  loopback idiom for system audio, plus normal WASAPI capture for the
  microphone, both shared-mode with `autoconvert` (resampling/downmixing handled
  by the audio engine, not manually). Verified against real hardware on this
  machine: both backends captured real, audible samples at the correct rate
  while audio was actually playing (hardware-dependent tests, `#[ignore]`d by
  default, not run in CI).
- Linux backend (compiles under CI's Linux leg; **not** validated against real
  hardware - no Linux audio stack available in this environment): system audio
  via the default sink's PulseAudio/PipeWire monitor source
  (`libpulse-simple-binding`, device resolved via `pactl get-default-sink`),
  microphone via the same blocking `Simple` API against the default source.
- `nastro-tap` is now a real capture helper, not a stub: spawns both
  requested sources, mixes them on the `SampleMixer` schedule, writes real audio
  incrementally to disk, reports `level sys=.. mic=..` once a second, and
  responds to the Ctrl+C -> graceful-stop contract from Phase 2.
- **Interim WAV writer** (`bin/nastro-tap/src/wav.rs`): 16-bit PCM, header
  rewritten once a second (not just at the very end) so a hard kill loses at
  most ~1s of "this file's data size" accuracy rather than leaving the whole
  file looking empty - a stopgap for real crash safety pending Phase 4's
  fragmented-MP4/`ffmpeg` muxing, which doesn't need this trick at all.
  Verified end-to-end: forcibly killed `nastro-tap` mid-recording (simulating a
  crash) while real audio played, and the resulting file's header matched its
  actual written data within the 1s flush bound, with `metadata.json` present at
  its own (upstream-matching) 5s cadence.

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

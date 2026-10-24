//! End-to-end TUI tests: the real `nastro` binary running inside a real
//! pseudo-terminal (ConPTY on Windows, a Unix pty elsewhere), driven with real
//! keystrokes, with the screen read back through a terminal emulator (`vt100`).
//!
//! These cover what the unit tests can't: raw-mode setup, alternate-screen
//! enter/leave, real key decoding through crossterm, and terminal restore on exit.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};

const ROWS: u16 = 24;
const COLS: u16 = 100;
const WAIT: Duration = Duration::from_secs(15);

struct Term {
    screen: Arc<Mutex<vt100::Parser>>,
    raw: Arc<Mutex<Vec<u8>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    // Keep the master alive for the lifetime of the session.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Term {
    fn launch(config: &Path) -> Self {
        Self::launch_with_args(config, &[])
    }

    fn launch_with_args(config: &Path, args: &[&str]) -> Self {
        Self::launch_with(config, args, &[])
    }

    fn launch_with(config: &Path, args: &[&str], envs: &[(&str, &Path)]) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");

        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_nastro"));
        cmd.args(args);
        cmd.env("NASTRO_CONFIG", config);
        // Hermetic: a developer's own Hugging Face token (in their user environment, which the pseudo-terminal
        // inherits) must never decide what these tests do, nor reach the program under test.
        cmd.env_remove("HF_TOKEN");
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd).expect("spawn nastro");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("clone reader");
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer().expect("take writer")));

        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            std::num::NonZeroU16::new(ROWS).unwrap(),
            std::num::NonZeroU16::new(COLS).unwrap(),
            0,
        )));
        let raw = Arc::new(Mutex::new(Vec::new()));
        {
            let (screen, raw, writer) =
                (Arc::clone(&screen), Arc::clone(&raw), Arc::clone(&writer));
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    // Act as the terminal: Windows ConPTY sends a cursor-position
                    // query (ESC[6n) at startup and withholds all output until it is
                    // answered, exactly as it would for a real terminal emulator.
                    if buf[..n].windows(4).any(|w| w == b"\x1b[6n") {
                        let mut w = writer.lock().unwrap();
                        let _ = w.write_all(b"\x1b[1;1R");
                        let _ = w.flush();
                    }
                    screen.lock().unwrap().process(&buf[..n]);
                    raw.lock().unwrap().extend_from_slice(&buf[..n]);
                }
            });
        }

        Self {
            screen,
            raw,
            writer,
            child,
            _master: pair.master,
        }
    }

    fn contents(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    fn send(&mut self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes).expect("write to pty");
        w.flush().unwrap();
    }

    /// Wait until `pred(screen)` holds; on timeout fail with the screen contents so
    /// the failure is diagnosable.
    fn wait_until(&self, what: &str, pred: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            let c = self.contents();
            if pred(&c) {
                return;
            }
            if Instant::now() >= deadline {
                let raw = self.raw.lock().unwrap();
                panic!(
                    "timed out waiting for {what}. Screen was:\n{c}\n--- {} raw bytes: {:?}",
                    raw.len(),
                    String::from_utf8_lossy(&raw[..raw.len().min(400)])
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_for(&self, needle: &str) {
        self.wait_until(&format!("{needle:?}"), |c| c.contains(needle));
    }

    fn wait_exit(&mut self) -> bool {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status.success();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("nastro did not exit. Screen was:\n{}", self.contents());
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn seed(root: &Path) -> PathBuf {
    for (name, transcript) in [
        (
            "2026-10-22-1430-standup",
            Some("we agreed to ship on friday"),
        ),
        ("2026-10-21-1000-planning", None),
        ("2026-10-20-0900-retro", None),
    ] {
        let dir = root.join("recordings").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audio.m4a"), vec![0u8; 2048]).unwrap();
        std::fs::write(dir.join("metadata.json"), r#"{"duration_seconds": 125.0}"#).unwrap();
        if let Some(t) = transcript {
            std::fs::write(dir.join("transcript.txt"), t).unwrap();
        }
    }
    let config = root.join("config.toml");
    // TOML literal string (single quotes): no escaping of Windows backslashes.
    std::fs::write(
        &config,
        format!("output_dir = '{}'\n", root.join("recordings").display()),
    )
    .unwrap();
    config
}

#[test]
fn browse_help_filter_cancel_delete_then_quit_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let config = seed(tmp.path());
    let mut t = Term::launch(&config);

    // The list renders the seeded recordings.
    t.wait_for("3 recordings");
    let c = t.contents();
    assert!(c.contains("2026-10-22-1430-standup"), "{c}");
    assert!(c.contains("2026-10-21-1000-planning"), "{c}");
    assert!(c.contains("2:05"), "duration column missing:\n{c}");

    // Help opens, then any key closes it.
    t.send(b"?");
    t.wait_for("Keys");
    t.send(b"z");
    t.wait_until("help to close", |c| !c.contains("Keys"));

    // Filtering narrows the list; typing 'q' while filtering must NOT quit.
    t.send(b"/plan");
    t.wait_until("filter to narrow", |c| {
        c.contains("planning") && !c.contains("standup")
    });
    // Esc cancels the filter and restores the full list.
    t.send(b"\x1b");
    t.wait_until("filter to clear", |c| c.contains("standup"));

    // Delete asks first; declining leaves the recording on disk.
    t.send(b"d");
    t.wait_for("Delete recording?");
    t.send(b"n");
    t.wait_until("modal to close", |c| !c.contains("Delete recording?"));
    assert!(
        tmp.path()
            .join("recordings/2026-10-22-1430-standup")
            .exists(),
        "declined delete must not remove anything"
    );

    // Quit exits 0 and leaves the alternate screen (terminal restored).
    t.send(b"q");
    assert!(t.wait_exit(), "nastro should exit successfully");
    let raw = t.raw.lock().unwrap().clone();
    let raw = String::from_utf8_lossy(&raw);
    assert!(
        raw.contains("\x1b[?1049h"),
        "should have entered the alternate screen"
    );
    assert!(
        raw.contains("\x1b[?1049l"),
        "should have left the alternate screen on exit"
    );
}

#[test]
fn enter_opens_detail_with_the_transcript_preview() {
    let tmp = tempfile::tempdir().unwrap();
    let config = seed(tmp.path());
    let mut t = Term::launch(&config);

    t.wait_for("3 recordings");
    t.send(b"\r");
    t.wait_for("Transcript preview");
    let c = t.contents();
    assert!(c.contains("we agreed to ship on friday"), "{c}");
    assert!(c.contains("2026-10-22-1430-standup"), "{c}");

    // Esc goes back to the list.
    t.send(b"\x1b");
    t.wait_until("return to list", |c| !c.contains("Transcript preview"));

    t.send(b"q");
    assert!(t.wait_exit());
}

#[test]
fn an_empty_recordings_folder_shows_guidance() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "output_dir = '{}'\n",
            tmp.path().join("nothing-here").display()
        ),
    )
    .unwrap();
    let mut t = Term::launch(&config);

    t.wait_for("No recordings yet");
    t.wait_for("Press r to record");
    t.send(b"q");
    assert!(t.wait_exit());
}

/// Real recording through the TUI. Needs an audio input device, ffmpeg on PATH and a
/// built `nastro-tap`, so it is ignored by default. Run locally with:
///   cargo build --workspace
///   cargo test -p nastro --test tui_pty -- --ignored --nocapture
#[test]
#[ignore]
fn live_record_a_few_seconds_with_the_mic_then_stop_and_save() {
    let tmp = tempfile::tempdir().unwrap();
    let config = seed(tmp.path());
    let mut t = Term::launch(&config);

    t.wait_for("3 recordings");
    t.send(b"r");
    t.wait_for("New recording");
    t.send(b"pty live");
    // Tab twice: Mixed -> Mic only -> System only is one step each; one Tab = mic only.
    t.send(b"\t");
    t.wait_until("mic-only selected", |c| c.contains("\u{25c9} Mic only"));
    t.send(b"\r");

    t.wait_for("REC");
    t.wait_until("timer to advance", |c| {
        c.contains("0:02") || c.contains("0:03")
    });
    t.send(b"s");
    t.wait_for("Saved");

    t.wait_until("new recording listed", |c| c.contains("pty-live"));
    let dir = std::fs::read_dir(tmp.path().join("recordings"))
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().contains("pty-live"))
        .expect("recording dir on disk")
        .path();
    let size = std::fs::metadata(dir.join("audio.m4a")).unwrap().len();
    assert!(size > 1_000, "audio.m4a only {size} bytes");

    t.send(b"q");
    assert!(t.wait_exit());
}

/// Discarding a live recording must leave nothing behind - not even the empty
/// directory - because the capture and ffmpeg processes may still hold the files
/// open at the moment of deletion. Same prerequisites as the test above.
#[test]
#[ignore]
fn live_discard_a_recording_removes_it_completely() {
    let tmp = tempfile::tempdir().unwrap();
    let config = seed(tmp.path());
    let mut t = Term::launch(&config);

    t.wait_for("3 recordings");
    t.send(b"r");
    t.wait_for("New recording");
    t.send(b"throwaway\t\r");

    t.wait_for("REC");
    t.wait_until("timer to advance", |c| {
        c.contains("0:02") || c.contains("0:03")
    });
    t.send(b"x");
    t.wait_for("Discard recording?");
    t.send(b"y");
    t.wait_for("Recording discarded");

    let leftovers: Vec<_> = std::fs::read_dir(tmp.path().join("recordings"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("throwaway") || n.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "discard left behind: {leftovers:?}");

    t.send(b"q");
    assert!(t.wait_exit());
}

/// The headless `nastro record` command: Ctrl+C must stop, finalize and save the
/// recording, release the lock, and exit 0. Same prerequisites as the live tests above.
#[test]
#[ignore]
fn live_headless_record_stops_and_saves_on_ctrl_c() {
    let tmp = tempfile::tempdir().unwrap();
    let config = seed(tmp.path());
    let mut t = Term::launch_with_args(&config, &["record", "--mic-only", "--name", "headless"]);

    t.wait_for("Press Ctrl+C to stop");
    t.wait_until("timer to advance", |c| c.contains("00:03"));
    t.send(&[0x03]); // Ctrl+C
    t.wait_for("Saved");
    assert!(t.wait_exit(), "should exit 0 after a clean save");

    let recordings = tmp.path().join("recordings");
    let dir = std::fs::read_dir(&recordings)
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().contains("headless"))
        .expect("recording dir on disk")
        .path();
    let size = std::fs::metadata(dir.join("audio.m4a")).unwrap().len();
    assert!(size > 1_000, "audio.m4a only {size} bytes");
    assert!(!recordings.join(".lock").exists(), "lock must be released");
}

/// A terminal being closed (SIGHUP) or the process being asked to stop (SIGTERM) must
/// shut the TUI down cleanly and give the terminal back, not just die in raw mode on
/// the alternate screen. Unix-only: it uses `kill`.
#[cfg(unix)]
#[test]
fn sigterm_exits_cleanly_and_restores_the_terminal() {
    for signal in ["-TERM", "-HUP"] {
        let tmp = tempfile::tempdir().unwrap();
        let config = seed(tmp.path());
        let mut t = Term::launch(&config);

        t.wait_for("3 recordings");
        let pid = t.child.process_id().expect("child pid");
        let status = std::process::Command::new("kill")
            .arg(signal)
            .arg(pid.to_string())
            .status()
            .expect("run kill");
        assert!(status.success(), "kill {signal} failed");

        assert!(t.wait_exit(), "should exit 0 after {signal}");
        let raw = String::from_utf8_lossy(&t.raw.lock().unwrap()).into_owned();
        assert!(
            raw.contains("\x1b[?1049l"),
            "{signal}: must leave the alternate screen"
        );
    }
}

// ------------------------------------------------------------------ transcription

/// Like `seed`, but with a chosen config body appended (e.g. a model or transcriber).
fn seed_with(root: &Path, extra_config: &str) -> PathBuf {
    let config = seed(root);
    let mut body = std::fs::read_to_string(&config).unwrap();
    body.push_str(extra_config);
    std::fs::write(&config, body).unwrap();
    config
}

#[test]
fn t_asks_before_downloading_a_missing_model_and_declining_downloads_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let models = tmp.path().join("models"); // empty: the model is not installed
    std::fs::create_dir_all(&models).unwrap();
    let config = seed_with(tmp.path(), "whisper_model = 'tiny.en'\n");
    let mut t = Term::launch_with(&config, &[], &[("NASTRO_MODEL_DIR", &models)]);

    t.wait_for("3 recordings");
    t.send(b"j"); // 2026-10-21-1000-planning: no transcript yet
    t.send(b"t");
    t.wait_for("Download speech model?");
    let c = t.contents();
    assert!(c.contains("tiny.en") && c.contains("huggingface.co"), "{c}");

    t.send(b"n");
    t.wait_until("dialog to close", |c| !c.contains("Download speech model?"));
    assert_eq!(
        std::fs::read_dir(&models).unwrap().count(),
        0,
        "declining must not download (or start downloading) anything"
    );
    assert!(!tmp
        .path()
        .join("recordings/2026-10-21-1000-planning/transcript.txt")
        .exists());

    t.send(b"q");
    assert!(t.wait_exit(), "no job is running, so q quits straight away");
}

#[test]
fn an_unknown_transcriber_is_explained_in_the_status_bar() {
    let tmp = tempfile::tempdir().unwrap();
    let config = seed_with(tmp.path(), "transcriber = 'deepgram'\n");
    let mut t = Term::launch(&config);

    t.wait_for("3 recordings");
    t.send(b"j");
    t.send(b"t");
    t.wait_for("deepgram");
    t.send(b"q");
    assert!(t.wait_exit());
}

#[test]
fn speaker_labels_without_a_token_explain_how_to_get_one() {
    let tmp = tempfile::tempdir().unwrap();
    let config = seed_with(tmp.path(), "transcriber = 'whisperx'\n");
    let mut t = Term::launch_with(&config, &[], &[]);

    t.wait_for("3 recordings");
    t.send(b"j");
    t.send(b"t");
    t.wait_for("Hugging Face");
    assert!(
        !tmp.path()
            .join("recordings/2026-10-21-1000-planning/transcript.txt")
            .exists(),
        "nothing may have started"
    );
    t.send(b"q");
    assert!(t.wait_exit(), "no job is running, so q quits straight away");
}

fn ffmpeg_program() -> std::ffi::OsString {
    std::env::var_os("NASTRO_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

fn ffmpeg_ok() -> bool {
    std::process::Command::new(ffmpeg_program())
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The whole feature through the real TUI: press `t`, watch it run in the background, see the transcript. Needs
/// ffmpeg and a model, so like the library's end-to-end tests it only runs when asked
/// (`NASTRO_TEST_WHISPER_MODEL=<file>` or `NASTRO_TEST_DOWNLOAD_MODEL=1`), and `NASTRO_REQUIRE_WHISPER=1` turns a
/// skip into a failure.
#[test]
fn t_transcribes_in_the_background_and_the_transcript_appears() {
    let require = std::env::var_os("NASTRO_REQUIRE_WHISPER").is_some();
    let model: PathBuf = if let Some(p) = std::env::var_os("NASTRO_TEST_WHISPER_MODEL") {
        p.into()
    } else if std::env::var_os("NASTRO_TEST_DOWNLOAD_MODEL").is_some() {
        nastro_transcribe::models::ensure(
            "tiny.en",
            &nastro_transcribe::CancelToken::new(),
            &mut |_, _| {},
        )
        .expect("model download")
    } else {
        assert!(
            !require,
            "NASTRO_REQUIRE_WHISPER is set but no model was provided"
        );
        eprintln!("skipping: set NASTRO_TEST_WHISPER_MODEL or NASTRO_TEST_DOWNLOAD_MODEL=1");
        return;
    };
    if !ffmpeg_ok() {
        assert!(
            !require,
            "NASTRO_REQUIRE_WHISPER is set but ffmpeg is missing"
        );
        eprintln!("skipping: ffmpeg is not installed");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let config = seed_with(
        tmp.path(),
        &format!("whisper_model = '{}'\nlang = 'en'\n", model.display()),
    );
    // A real recording (newest, so it is the first row): the public-domain JFK sample as AAC in MP4.
    let name = "2026-10-23-0900-speech";
    let dir = tmp.path().join("recordings").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let wav = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/nastro-transcribe/tests/fixtures/jfk.wav");
    let status = std::process::Command::new(ffmpeg_program())
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&wav)
        .args(["-ac", "1", "-c:a", "aac", "-b:a", "64k", "-y"])
        .arg(dir.join("audio.m4a"))
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::write(dir.join("metadata.json"), r#"{"duration_seconds": 11.0}"#).unwrap();

    let mut t = Term::launch(&config);
    t.wait_for("4 recordings");
    t.send(b"t"); // the newest recording is selected
    t.wait_for(&format!("Transcribed {name}"));

    let txt = std::fs::read_to_string(dir.join("transcript.txt")).unwrap();
    assert!(
        txt.to_lowercase()
            .contains("ask not what your country can do for you"),
        "{txt:?}"
    );
    assert!(dir.join("transcript.srt").exists());
    // Opening the recording shows the new transcript.
    t.send(b"\r");
    t.wait_for("Transcript preview");
    t.wait_for("your country");

    // Wait for the screen to change between Esc and q: sent together, a Unix terminal reads them as Alt+q.
    t.send(b"\x1b");
    t.wait_until("return to list", |c| !c.contains("Transcript preview"));
    t.send(b"q");
    assert!(t.wait_exit());
}

/// The stand-in `whisperx` built by `nastro-transcribe`, if this run built it (a workspace test run does; a run of
/// only this package may not).
fn fake_whisperx() -> Option<PathBuf> {
    let exe = Path::new(env!("CARGO_BIN_EXE_nastro"));
    let name = if cfg!(windows) {
        "nastro-fake-whisperx.exe"
    } else {
        "nastro-fake-whisperx"
    };
    let p = exe.parent()?.join(name);
    p.is_file().then_some(p)
}

/// Speaker labels through the real TUI: `t` runs the (stand-in) WhisperX in the background and the labelled
/// transcript appears. Needs ffmpeg; skipped without the stand-in unless `NASTRO_REQUIRE_WHISPER` is set.
#[test]
fn t_with_speaker_labels_runs_whisperx_in_the_background() {
    let require = std::env::var_os("NASTRO_REQUIRE_WHISPER").is_some();
    let Some(fake) = fake_whisperx() else {
        assert!(
            !require,
            "NASTRO_REQUIRE_WHISPER is set but nastro-fake-whisperx was not built"
        );
        eprintln!("skipping: nastro-fake-whisperx not built (run `cargo test --workspace`)");
        return;
    };
    if !ffmpeg_ok() {
        assert!(
            !require,
            "NASTRO_REQUIRE_WHISPER is set but ffmpeg is missing"
        );
        eprintln!("skipping: ffmpeg is not installed");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let config = seed_with(
        tmp.path(),
        "transcriber = 'whisperx'\nhf_token = 'hf_notarealtokenjustatestvalue0'\nwhisper_model = 'small'\nlang = 'en'\n",
    );
    let name = "2026-10-23-0900-meeting";
    let dir = tmp.path().join("recordings").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let status = std::process::Command::new(ffmpeg_program())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=16000:cl=mono",
            "-t",
            "2",
        ])
        .args(["-ac", "1", "-c:a", "aac", "-b:a", "64k", "-y"])
        .arg(dir.join("audio.m4a"))
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::write(dir.join("metadata.json"), r#"{"duration_seconds": 2.0}"#).unwrap();

    let mut t = Term::launch_with(&config, &[], &[("NASTRO_WHISPERX", fake.as_path())]);
    t.wait_for("4 recordings");
    t.send(b"t"); // the newest recording is selected
    t.wait_for(&format!("Transcribed {name}"));

    let txt = std::fs::read_to_string(dir.join("transcript.txt")).unwrap();
    assert!(
        txt.starts_with("[SPEAKER_00] Good morning everyone."),
        "{txt:?}"
    );
    assert!(txt.contains("[SPEAKER_01] Thanks."), "{txt:?}");
    t.send(b"\r");
    t.wait_for("Transcript preview");
    t.wait_for("[SPEAKER_00]");

    t.send(b"\x1b");
    t.wait_until("return to list", |c| !c.contains("Transcript preview"));
    t.send(b"q");
    assert!(t.wait_exit());
}

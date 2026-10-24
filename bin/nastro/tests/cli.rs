//! Non-interactive CLI behaviour: config selection and the plain listing. (The
//! full-screen UI is covered by `tui_pty.rs`.)

use std::path::Path;
use std::process::{Command, Output};

fn nastro(config_arg: Option<&Path>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nastro"));
    // Never let the developer's or CI's own environment pick the config.
    cmd.env_remove("NASTRO_CONFIG");
    if let Some(p) = config_arg {
        cmd.arg("--config").arg(p);
    }
    cmd.args(args).output().expect("run nastro")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_missing_explicit_config_is_an_error_not_a_silent_default() {
    let tmp = tempfile::tempdir().unwrap();
    let out = nastro(Some(&tmp.path().join("typo.toml")), &["records", "--plain"]);

    assert!(
        !out.status.success(),
        "must fail, stdout: {}",
        text(&out.stdout)
    );
    let err = text(&out.stderr);
    assert!(err.contains("config file not found"), "stderr: {err}");
    assert!(err.contains("typo.toml"), "should name the file: {err}");
}

#[test]
fn an_unparseable_explicit_config_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("bad.toml");
    std::fs::write(&bad, "this is = [not valid").unwrap();

    let out = nastro(Some(&bad), &["records", "--plain"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("failed to parse config file"));
}

#[test]
fn the_nastro_config_env_var_is_honoured_and_checked_the_same_way() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_nastro"))
        .env("NASTRO_CONFIG", tmp.path().join("nope.toml"))
        .args(["records", "--plain"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("config file not found"));
}

#[test]
fn a_valid_explicit_config_selects_the_recordings_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = tmp.path().join("recs").join("2026-10-20-0900-standup");
    std::fs::create_dir_all(&rec).unwrap();
    std::fs::write(rec.join("audio.m4a"), vec![0u8; 1024]).unwrap();
    std::fs::write(rec.join("metadata.json"), r#"{"duration_seconds": 65.0}"#).unwrap();

    let cfg = tmp.path().join("config.toml");
    // TOML literal string: no escaping needed for Windows backslashes.
    std::fs::write(
        &cfg,
        format!("output_dir = '{}'\n", tmp.path().join("recs").display()),
    )
    .unwrap();

    let out = nastro(Some(&cfg), &["records", "--plain"]);
    assert!(out.status.success(), "stderr: {}", text(&out.stderr));
    let line = text(&out.stdout);
    assert!(line.contains("2026-10-20-0900-standup"), "{line}");
    assert!(line.contains("1:05"), "{line}");
    assert!(line.contains('\t'), "plain output is tab-separated: {line}");
}

/// `nastro records --plain | head` closes the pipe while nastro is still printing. That must end quietly (like any
/// Unix tool), not panic with "failed printing to stdout: Broken pipe" and exit 101, which is what `println!` does.
#[test]
fn a_closed_pipe_ends_a_listing_quietly_instead_of_panicking() {
    use std::io::Read;
    use std::process::Stdio;

    let tmp = tempfile::tempdir().unwrap();
    let recordings = tmp.path().join("recordings");
    // Far more output than any pipe buffer holds, so the writer is certain to hit the closed pipe.
    for i in 0..4000 {
        std::fs::create_dir_all(recordings.join(format!("2020-01-01-0900-rec{i}"))).unwrap();
    }
    let config = tmp.path().join("config.toml");
    std::fs::write(
        &config,
        format!("output_dir = '{}'\n", recordings.display()),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_nastro"))
        .env_remove("NASTRO_CONFIG")
        .arg("--config")
        .arg(&config)
        .args(["records", "--plain"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn nastro");

    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 64];
    stdout.read_exact(&mut first).expect("some output");
    drop(stdout); // the reader goes away, like `| head`

    let status = child.wait().expect("wait");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();

    assert!(
        !stderr.contains("panicked"),
        "panicked on a closed pipe:\n{stderr}"
    );
    assert!(status.success(), "exit status {status:?}\n{stderr}");
}

// ------------------------------------------------------------------------ speaker labels (WhisperX)

/// A config + one recording directory (real or placeholder audio), for `nastro transcribe` tests.
fn transcribe_fixture(extra_config: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let rec = tmp
        .path()
        .join("recordings")
        .join("2026-10-23-0900-meeting");
    std::fs::create_dir_all(&rec).unwrap();
    std::fs::write(rec.join("audio.m4a"), vec![0u8; 2048]).unwrap();
    let config = tmp.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "output_dir = '{}'\n{extra_config}",
            tmp.path().join("recordings").display()
        ),
    )
    .unwrap();
    (tmp, config)
}

fn transcribe(config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nastro"))
        .env_remove("NASTRO_CONFIG")
        .env_remove("HF_TOKEN")
        .env_remove("NASTRO_WHISPERX")
        .arg("--config")
        .arg(config)
        .arg("transcribe")
        .args(args)
        .output()
        .expect("run nastro")
}

#[test]
fn speaker_count_options_need_the_speaker_engine() {
    let (_tmp, config) = transcribe_fixture("");
    let out = transcribe(&config, &["last", "--speakers", "2"]);
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(err.contains("--transcriber whisperx"), "{err}");
}

#[test]
fn nonsensical_speaker_counts_are_rejected() {
    let (_tmp, config) = transcribe_fixture("");
    for (args, want) in [
        (
            vec!["last", "--transcriber", "whisperx", "--speakers", "0"],
            "at least 1",
        ),
        (
            vec![
                "last",
                "--transcriber",
                "whisperx",
                "--min-speakers",
                "4",
                "--max-speakers",
                "2",
            ],
            "cannot be larger",
        ),
    ] {
        let out = transcribe(&config, &args);
        assert!(!out.status.success(), "{args:?}");
        assert!(
            text(&out.stderr).contains(want),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    // --speakers and --min/--max are alternatives, so clap refuses both together.
    let out = transcribe(
        &config,
        &[
            "last",
            "--transcriber",
            "whisperx",
            "--speakers",
            "2",
            "--max-speakers",
            "3",
        ],
    );
    assert!(!out.status.success());
}

#[test]
fn an_unknown_engine_name_is_an_error() {
    let (_tmp, config) = transcribe_fixture("");
    let out = transcribe(&config, &["last", "--transcriber", "deepgram"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("deepgram"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn speaker_labels_without_a_token_explain_the_three_steps_and_never_start() {
    let (_tmp, config) = transcribe_fixture("transcriber = 'whisperx'\n");
    let out = transcribe(&config, &["last", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    for want in [
        "huggingface.co/settings/tokens",
        "speaker-diarization-community-1",
        "hf_token",
    ] {
        assert!(err.contains(want), "missing {want:?}:\n{err}");
    }
}

#[test]
fn setup_needs_no_recording_id() {
    // Without uv or whisperx installed this fails, but it must get as far as trying, not complain about a
    // missing ID. (The error text varies with the machine, so only the argument handling is asserted.)
    let (_tmp, config) = transcribe_fixture("");
    let out = Command::new(env!("CARGO_BIN_EXE_nastro"))
        .env_remove("NASTRO_CONFIG")
        .env("PATH", "")
        .env("NASTRO_WHISPERX", "does-not-exist")
        .arg("--config")
        .arg(&config)
        .args(["transcribe", "--setup-whisperx"])
        .output()
        .unwrap();
    let err = text(&out.stderr);
    assert!(!err.contains("required arguments"), "{err}");
}

fn fake_whisperx() -> Option<std::path::PathBuf> {
    let exe = Path::new(env!("CARGO_BIN_EXE_nastro"));
    let name = if cfg!(windows) {
        "nastro-fake-whisperx.exe"
    } else {
        "nastro-fake-whisperx"
    };
    let p = exe.parent()?.join(name);
    p.is_file().then_some(p)
}

/// A full `nastro transcribe` run through the stand-in WhisperX: labelled files and a summary that counts
/// speakers. Needs ffmpeg to make a real recording.
#[test]
fn a_whole_cli_run_with_speaker_labels() {
    let require = std::env::var_os("NASTRO_REQUIRE_WHISPER").is_some();
    let Some(fake) = fake_whisperx() else {
        assert!(
            !require,
            "NASTRO_REQUIRE_WHISPER is set but nastro-fake-whisperx was not built"
        );
        eprintln!("skipping: nastro-fake-whisperx not built (run `cargo test --workspace`)");
        return;
    };
    let ffmpeg = std::env::var_os("NASTRO_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    if !Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        assert!(
            !require,
            "NASTRO_REQUIRE_WHISPER is set but ffmpeg is missing"
        );
        eprintln!("skipping: ffmpeg is not installed");
        return;
    }
    let (tmp, config) = transcribe_fixture("transcriber = 'whisperx'\nhf_token = 'hf_notarealtokenjustatestvalue0'\nwhisper_model = 'small'\nlang = 'en'\n");
    let rec = tmp
        .path()
        .join("recordings")
        .join("2026-10-23-0900-meeting");
    let ok = Command::new(&ffmpeg)
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
        .arg(rec.join("audio.m4a"))
        .status()
        .unwrap()
        .success();
    assert!(ok);

    let out = Command::new(env!("CARGO_BIN_EXE_nastro"))
        .env_remove("NASTRO_CONFIG")
        .env("NASTRO_WHISPERX", &fake)
        .arg("--config")
        .arg(&config)
        .args(["transcribe", "last"])
        .output()
        .unwrap();

    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("2 speakers") && stdout.contains("transcript.txt"),
        "{stdout}"
    );
    let txt = std::fs::read_to_string(rec.join("transcript.txt")).unwrap();
    assert!(
        txt.starts_with("[SPEAKER_00] Good morning everyone."),
        "{txt}"
    );
    // Phases were shown on stderr (not a terminal here, so one line per stage).
    let err = text(&out.stderr);
    for want in ["Identifying speakers...", "Aligning words..."] {
        assert!(err.contains(want), "missing {want:?}:\n{err}");
    }
    // The token must not appear anywhere the user can see.
    assert!(!err.contains("hf_notarealtoken") && !stdout.contains("hf_notarealtoken"));
}

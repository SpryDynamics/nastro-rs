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

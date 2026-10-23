//! Live end-to-end test of the graceful-stop contract against the real
//! `nastro-tap` binary, real capture hardware and a real `ffmpeg`.
//!
//! `#[ignore]`d by default: it needs an audio device, ffmpeg on PATH, and a
//! prebuilt `nastro-tap` (`cargo build -p nastro-tap`). Run locally with:
//!
//!   cargo test -p nastro-record --test live_tap -- --ignored --nocapture

use std::path::PathBuf;
use std::time::Duration;

use nastro_record::{CaptureMode, TapExitKind, TapProcess};

fn tap_binary() -> PathBuf {
    let exe = if cfg!(windows) {
        "nastro-tap.exe"
    } else {
        "nastro-tap"
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug")
        .join(exe)
}

#[test]
#[ignore]
fn graceful_stop_finalizes_a_valid_m4a_and_exits_clean() {
    let tap = tap_binary();
    assert!(tap.is_file(), "build nastro-tap first: {}", tap.display());

    let dir = tempfile::tempdir().unwrap();
    let audio = dir.path().join("audio.m4a");

    // Mic-only so the test doesn't depend on something playing system audio;
    // silence still encodes to a valid file.
    let mut proc = TapProcess::spawn(&tap, &audio, CaptureMode::MicOnly).expect("spawn tap");

    std::thread::sleep(Duration::from_secs(3));
    proc.request_stop().expect("graceful stop signal");
    let desc = proc.wait_with_timeout(Duration::from_secs(10));

    assert_eq!(desc.kind, TapExitKind::Clean, "message: {}", desc.message);

    let bytes = std::fs::read(&audio).expect("audio file written");
    assert_eq!(&bytes[4..8], b"ftyp");
    assert!(bytes.windows(4).any(|w| w == b"moov"));
    assert!(bytes.len() > 2_000, "got only {} bytes", bytes.len());

    let meta = std::fs::read_to_string(dir.path().join("metadata.json")).expect("metadata.json");
    let value: serde_json::Value = serde_json::from_str(&meta).unwrap();
    let secs = value["duration_seconds"].as_f64().unwrap();
    assert!(
        (2.0..=4.5).contains(&secs),
        "unexpected duration {secs} for a ~3s recording"
    );

    // The strongest check that ffmpeg survived the stop signal and flushed its
    // final fragment: the *decoded* duration must match what was fed to it. If the
    // Ctrl+Break had also hit ffmpeg, the tail would be missing here.
    if let Ok(out) = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration"])
        .args(["-of", "default=nw=1:nk=1"])
        .arg(&audio)
        .output()
    {
        if out.status.success() {
            let decoded: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
            assert!(
                (decoded - secs).abs() < 0.25,
                "decoded {decoded}s vs recorded {secs}s: tail was lost at shutdown"
            );
        }
    }
}

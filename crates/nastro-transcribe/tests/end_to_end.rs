//! End-to-end: real ffmpeg, a real whisper model, real speech.
//!
//! These need a model and ffmpeg, so they only run when asked to:
//!
//! * `NASTRO_TEST_WHISPER_MODEL=<path to a ggml .bin>` uses that model, or
//! * `NASTRO_TEST_DOWNLOAD_MODEL=1` downloads (and verifies) `tiny.en` into the model cache first.
//!
//! With `NASTRO_REQUIRE_WHISPER=1` (set in CI) a missing model/ffmpeg is a failure instead of a skip, so the
//! pipeline cannot quietly stop testing the real thing.
//!
//! The speech is `fixtures/jfk.wav`: an 11-second excerpt of a 1961 US presidential address (public domain,
//! the sample whisper.cpp ships for the same purpose).

use std::path::{Path, PathBuf};
use std::process::Command;

use nastro_transcribe::{transcribe, CancelToken, Error, Options, Progress};

const EXPECTED_PHRASE: &str = "ask not what your country can do for you";

fn ffmpeg() -> std::ffi::OsString {
    std::env::var_os("NASTRO_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

fn have_ffmpeg() -> bool {
    Command::new(ffmpeg())
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The model to test with, or `None` (after saying why) when this run should skip.
fn model_or_skip() -> Option<String> {
    let require = std::env::var_os("NASTRO_REQUIRE_WHISPER").is_some();
    let skip = |why: &str| -> Option<String> {
        assert!(!require, "NASTRO_REQUIRE_WHISPER is set but: {why}");
        eprintln!("skipping: {why}");
        None
    };
    if !have_ffmpeg() {
        return skip("ffmpeg is not installed");
    }
    if let Some(p) = std::env::var_os("NASTRO_TEST_WHISPER_MODEL") {
        return Some(p.to_string_lossy().into_owned());
    }
    if std::env::var_os("NASTRO_TEST_DOWNLOAD_MODEL").is_some() {
        let path =
            nastro_transcribe::models::ensure("tiny.en", &CancelToken::new(), &mut |_, _| {})
                .expect("downloading and verifying tiny.en");
        return Some(path.to_string_lossy().into_owned());
    }
    skip("set NASTRO_TEST_WHISPER_MODEL or NASTRO_TEST_DOWNLOAD_MODEL=1")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Encodes `input` into `<dir>/audio.m4a` exactly the way the recorder does (mono AAC 64 kbit/s, fragmented MP4).
fn make_recording(dir: &Path, input_args: &[&str]) {
    let status = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error"])
        .args(input_args)
        .args([
            "-ac",
            "1",
            "-c:a",
            "aac",
            "-b:a",
            "64k",
            "-f",
            "mp4",
            "-movflags",
            "+empty_moov+default_base_moof+frag_keyframe",
            "-frag_duration",
            "1000000",
            "-y",
        ])
        .arg(dir.join("audio.m4a"))
        .status()
        .expect("running ffmpeg");
    assert!(
        status.success(),
        "ffmpeg could not build the test recording"
    );
}

fn speech_recording() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    make_recording(dir.path(), &["-i", fixture("jfk.wav").to_str().unwrap()]);
    dir
}

fn options(model: &str) -> Options {
    Options {
        model: model.to_string(),
        lang: "auto".into(),
        threads: Some(4),
        ..Options::default()
    }
}

fn squash(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn transcribes_real_speech_end_to_end() {
    let Some(model) = model_or_skip() else { return };
    let dir = speech_recording();
    let mut stages = Vec::new();

    let out = transcribe(
        dir.path(),
        &options(&model),
        &CancelToken::new(),
        &mut |p| stages.push(p),
    )
    .unwrap();

    let txt = std::fs::read_to_string(dir.path().join("transcript.txt")).unwrap();
    assert!(
        squash(&txt).contains(EXPECTED_PHRASE),
        "unexpected transcript: {txt:?}"
    );
    let srt = std::fs::read_to_string(dir.path().join("transcript.srt")).unwrap();
    assert!(srt.starts_with("1\n00:00:0"), "{srt:?}");
    assert!(srt.contains(" --> "));

    assert_eq!(out.language, "en");
    assert!(out.segments >= 1);
    assert!(
        (out.audio_seconds - 11.0).abs() < 1.0,
        "{}",
        out.audio_seconds
    );

    // Stages arrive in order, and recognition reports progress up to 100%.
    assert!(stages.contains(&Progress::Decoding));
    assert!(stages.contains(&Progress::Transcribing { percent: 100 }));
    assert_eq!(stages.last(), Some(&Progress::Saving));
    let decode_at = stages
        .iter()
        .position(|s| *s == Progress::Decoding)
        .unwrap();
    let save_at = stages.iter().position(|s| *s == Progress::Saving).unwrap();
    assert!(decode_at < save_at);
}

#[test]
fn an_existing_transcript_needs_overwrite() {
    let Some(model) = model_or_skip() else { return };
    let dir = speech_recording();
    std::fs::write(dir.path().join("transcript.txt"), "old\n").unwrap();

    let e = transcribe(
        dir.path(),
        &options(&model),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(e, Error::AlreadyTranscribed(_)), "{e}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("transcript.txt")).unwrap(),
        "old\n"
    );

    let mut o = options(&model);
    o.overwrite = true;
    transcribe(dir.path(), &o, &CancelToken::new(), &mut |_| {}).unwrap();
    assert!(
        squash(&std::fs::read_to_string(dir.path().join("transcript.txt")).unwrap())
            .contains(EXPECTED_PHRASE)
    );
}

#[test]
fn cancelling_mid_run_keeps_the_previous_transcript_and_leaves_no_litter() {
    let Some(model) = model_or_skip() else { return };
    let dir = speech_recording();
    std::fs::write(dir.path().join("transcript.txt"), "previous\n").unwrap();
    let mut o = options(&model);
    o.overwrite = true;

    let cancel = CancelToken::new();
    let c = cancel.clone();
    let e = transcribe(dir.path(), &o, &cancel, &mut |p| {
        if matches!(p, Progress::Decoding) {
            c.cancel();
        }
    })
    .unwrap_err();

    assert!(matches!(e, Error::Cancelled), "{e}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("transcript.txt")).unwrap(),
        "previous\n"
    );
    assert!(!dir.path().join("transcript.srt").exists());
    let mut names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["audio.m4a", "transcript.txt"],
        "stray files: {names:?}"
    );
}

#[test]
fn silence_produces_an_empty_transcript_not_invented_words() {
    let Some(model) = model_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    make_recording(
        dir.path(),
        &["-f", "lavfi", "-i", "anullsrc=r=16000:cl=mono", "-t", "6"],
    );

    let out = transcribe(
        dir.path(),
        &options(&model),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();

    let txt = std::fs::read_to_string(dir.path().join("transcript.txt")).unwrap();
    assert_eq!(out.segments, 0, "hallucinated: {txt:?}");
    assert!(txt.trim().is_empty(), "{txt:?}");
}

#[test]
fn a_clip_shorter_than_one_second_does_not_fail() {
    let Some(model) = model_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    make_recording(
        dir.path(),
        &["-f", "lavfi", "-i", "anullsrc=r=16000:cl=mono", "-t", "0.4"],
    );
    transcribe(
        dir.path(),
        &options(&model),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
}

#[test]
fn a_corrupt_recording_is_a_clear_error_and_writes_nothing() {
    let Some(model) = model_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("audio.m4a"), b"this is not audio at all").unwrap();

    let e = transcribe(
        dir.path(),
        &options(&model),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();

    assert!(
        matches!(e, Error::FfmpegFailed(_) | Error::EmptyAudio),
        "{e}"
    );
    assert!(!dir.path().join("transcript.txt").exists());
}

#[test]
fn a_missing_model_file_is_reported_with_where_it_was_looked_for() {
    if !have_ffmpeg() {
        return;
    }
    let dir = speech_recording();
    let e = transcribe(
        dir.path(),
        &options(dir.path().join("no-such-model.bin").to_str().unwrap()),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(e, Error::ModelMissing { .. }), "{e}");
    assert!(e.to_string().contains("no-such-model.bin"));
}

#[test]
fn cancelling_during_recognition_stops_promptly_and_keeps_the_previous_transcript() {
    let Some(model) = model_or_skip() else { return };
    // ~4 minutes of speech, so recognition is still running when we cancel.
    let dir = tempfile::tempdir().unwrap();
    make_recording(
        dir.path(),
        &[
            "-stream_loop",
            "20",
            "-i",
            fixture("jfk.wav").to_str().unwrap(),
        ],
    );
    std::fs::write(dir.path().join("transcript.txt"), "previous\n").unwrap();
    let mut o = options(&model);
    o.overwrite = true;

    let cancel = CancelToken::new();
    let c = cancel.clone();
    let mut cancelled_at = None;
    let e = transcribe(dir.path(), &o, &cancel, &mut |p| {
        if let Progress::Transcribing { percent } = p {
            if percent >= 5 && cancelled_at.is_none() {
                cancelled_at = Some(std::time::Instant::now());
                c.cancel();
            }
        }
    })
    .unwrap_err();

    assert!(matches!(e, Error::Cancelled), "{e}");
    let took = cancelled_at
        .expect("recognition never reached 5%")
        .elapsed();
    assert!(
        took < std::time::Duration::from_secs(5),
        "cancel took {took:?} to take effect"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("transcript.txt")).unwrap(),
        "previous\n"
    );
    assert!(!dir.path().join("transcript.srt").exists());
}

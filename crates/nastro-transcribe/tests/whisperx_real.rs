//! The WhisperX integration against the *real* `whisperx` command.
//!
//! Needs WhisperX installed (`nastro transcribe --setup-whisperx`), ffmpeg and a network connection (WhisperX
//! downloads its small `tiny.en` and alignment models on first use), so it only runs when asked:
//!
//!     NASTRO_TEST_REAL_WHISPERX=1 cargo test -p nastro-transcribe --test whisperx_real -- --test-threads=1
//!
//! Speaker identification itself needs a Hugging Face token with access to the gated pyannote models, which this
//! test cannot assume; the diarization step is covered by `NASTRO_TEST_HF_TOKEN` (see `real_speaker_labels`).
//! Everything else in the pipeline (finding the tool, preparing audio, running, phases, parsing, atomic output,
//! cancelling and killing the process tree) is exercised here without one.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use nastro_transcribe::{
    transcribe, whisperx, Backend, CancelToken, Error, Options, Phase, Progress, Secret,
};

fn enabled() -> bool {
    if std::env::var_os("NASTRO_TEST_REAL_WHISPERX").is_none() {
        eprintln!("skipping: set NASTRO_TEST_REAL_WHISPERX=1 to run against the real whisperx");
        return false;
    }
    assert!(
        whisperx::locate().is_some(),
        "NASTRO_TEST_REAL_WHISPERX is set but whisperx is not installed (run `nastro transcribe --setup-whisperx`)"
    );
    true
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A recording directory whose `audio.m4a` is made from `wav` (repeated `loops` extra times, to make a long
/// recording), encoded as the recorder does.
fn recording_from(root: &Path, wav: &Path, loops: u32) -> PathBuf {
    let dir = root.join("rec");
    std::fs::create_dir_all(&dir).unwrap();
    let ffmpeg = std::env::var_os("NASTRO_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    let ok = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-stream_loop",
            &loops.to_string(),
            "-i",
        ])
        .arg(wav)
        .args(["-ac", "1", "-c:a", "aac", "-b:a", "64k", "-y"])
        .arg(dir.join("audio.m4a"))
        .status()
        .unwrap()
        .success();
    assert!(ok, "ffmpeg could not build the test recording");
    dir
}

fn options() -> Options {
    Options {
        backend: Backend::WhisperX,
        model: "tiny.en".into(),
        lang: "en".into(),
        // Without a Hugging Face account, run everything but the speaker step.
        diarize: false,
        threads: Some(4),
        overwrite: true,
        ..Options::default()
    }
}

#[test]
fn real_whisperx_transcribes_end_to_end_without_diarization() {
    if !enabled() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = recording_from(tmp.path(), &fixture("jfk.wav"), 0);
    let mut seen = Vec::new();

    let out = transcribe(&dir, &options(), &CancelToken::new(), &mut |p| seen.push(p))
        .expect("real whisperx run");

    let txt = std::fs::read_to_string(dir.join("transcript.txt")).unwrap();
    assert!(
        txt.to_lowercase()
            .contains("ask not what your country can do for you"),
        "{txt:?}"
    );
    assert!(
        !txt.contains("[SPEAKER"),
        "no diarization was requested: {txt:?}"
    );
    assert!(std::fs::read_to_string(dir.join("transcript.srt"))
        .unwrap()
        .contains(" --> "));
    assert_eq!(out.language, "en");
    assert!(
        (out.audio_seconds - 11.0).abs() < 1.0,
        "{}",
        out.audio_seconds
    );

    // The real tool's stage markers were recognised, in its order.
    let phases: Vec<Phase> = seen
        .iter()
        .filter_map(|p| {
            if let Progress::Phase(p) = p {
                Some(*p)
            } else {
                None
            }
        })
        .collect();
    assert!(phases.contains(&Phase::Transcribing), "{phases:?}");
    assert!(phases.contains(&Phase::Aligning), "{phases:?}");
    let t = phases
        .iter()
        .position(|p| *p == Phase::Transcribing)
        .unwrap();
    let a = phases.iter().position(|p| *p == Phase::Aligning).unwrap();
    assert!(t < a, "{phases:?}");
    assert_eq!(seen.last(), Some(&Progress::Saving));

    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["audio.m4a", "transcript.srt", "transcript.txt"],
        "no litter"
    );
}

/// Python processes that belong to the `uv`-installed WhisperX environment.
fn whisperx_python_pids() -> Vec<u32> {
    use sysinfo::{ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes()
        .values()
        .filter(|p| {
            p.exe()
                .map(|e| {
                    let e = e.to_string_lossy().to_ascii_lowercase().replace('\\', "/");
                    e.contains("/uv/tools/whisperx/")
                        || e.ends_with("/whisperx.exe")
                        || e.ends_with("/bin/whisperx")
                })
                .unwrap_or(false)
        })
        .map(|p| p.pid().as_u32())
        .collect()
}

/// The installed `whisperx.exe` launches Python as a child. Cancelling must stop *both*, promptly, or a cancelled
/// transcription keeps every CPU core busy in the background.
#[test]
fn real_cancel_stops_the_launcher_and_the_python_underneath() {
    if !enabled() {
        return;
    }
    assert!(
        whisperx_python_pids().is_empty(),
        "a WhisperX is already running; stop it first"
    );
    let tmp = tempfile::tempdir().unwrap();
    // About three minutes of audio, so a WhisperX that was *not* stopped would still be busy for far longer than
    // the grace period below (a short clip would simply finish by itself and the test would prove nothing).
    let dir = recording_from(tmp.path(), &fixture("jfk.wav"), 15);
    std::fs::write(dir.join("transcript.txt"), "previous\n").unwrap();

    let cancel = CancelToken::new();
    let c = cancel.clone();
    let mut running_at_cancel = 0;
    let mut cancelled_at = None;
    let r = transcribe(&dir, &options(), &cancel, &mut |p| {
        // Only once it is transcribing: before that it has not loaded the audio yet, and would crash on its own
        // when its temporary folder is removed, which would hide a failure to stop it.
        if matches!(p, Progress::Phase(Phase::Transcribing)) && cancelled_at.is_none() {
            running_at_cancel = whisperx_python_pids().len();
            cancelled_at = Some(Instant::now());
            c.cancel();
        }
    });

    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    assert!(
        running_at_cancel > 0,
        "the test never saw WhisperX running, so it proves nothing"
    );
    let deadline = Instant::now() + Duration::from_secs(4);
    while !whisperx_python_pids().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let left = whisperx_python_pids();
    assert!(
        left.is_empty(),
        "WhisperX processes were left running after cancel: {left:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("transcript.txt")).unwrap(),
        "previous\n"
    );
}

/// The speaker step proper. Needs a Hugging Face token that has accepted the pyannote terms, and a recording with
/// at least two people in it (`NASTRO_TEST_DIALOGUE`, a WAV): it is the one thing that cannot run unattended.
#[test]
fn real_speaker_labels() {
    let (Some(token), Some(wav)) = (
        std::env::var("NASTRO_TEST_HF_TOKEN")
            .ok()
            .and_then(Secret::new),
        std::env::var_os("NASTRO_TEST_DIALOGUE").map(PathBuf::from),
    ) else {
        eprintln!("skipping: set NASTRO_TEST_HF_TOKEN and NASTRO_TEST_DIALOGUE=<two-speaker .wav> (with NASTRO_TEST_REAL_WHISPERX=1)");
        return;
    };
    if !enabled() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = recording_from(tmp.path(), &wav, 0);
    let mut o = options();
    o.diarize = true;
    o.hf_token = Some(token);
    o.speakers = nastro_transcribe::Speakers::exactly(2);

    let out = transcribe(&dir, &o, &CancelToken::new(), &mut |_| {})
        .expect("real speaker identification");

    let txt = std::fs::read_to_string(dir.join("transcript.txt")).unwrap();
    println!("{txt}");
    assert_eq!(out.speakers, 2, "two voices, two labels:\n{txt}");
    assert!(
        txt.contains("[SPEAKER_00]") && txt.contains("[SPEAKER_01]"),
        "{txt}"
    );
}

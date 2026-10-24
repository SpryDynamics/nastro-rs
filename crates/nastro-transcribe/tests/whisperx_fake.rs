//! The WhisperX integration, exercised against `nastro-fake-whisperx` (a stand-in that behaves like the real
//! command, see `src/bin/fake_whisperx.rs`). Real ffmpeg is used to prepare the audio.
//!
//! This covers everything nastro does *around* WhisperX: argument and environment handling, the Hugging Face
//! token, phase reporting, result parsing and writing, every failure mapping, cancellation and cleanup. It
//! needs no Python and no account, so it runs in CI. The real tool is covered by `whisperx_real.rs`.
//!
//! The tests share process-wide environment variables, so they take a lock and run one at a time.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use nastro_transcribe::{
    transcribe, Backend, CancelToken, Error, Options, Phase, Progress, Secret, Speakers,
};

static LOCK: Mutex<()> = Mutex::new(());

const TOKEN: &str = "hf_TESTTOKENabcdefghijklmnop1234";

fn ffmpeg() -> std::ffi::OsString {
    std::env::var_os("NASTRO_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

/// Holds the lock and the environment for one test; removes what it set when dropped.
struct Fake {
    _guard: MutexGuard<'static, ()>,
    set: Vec<&'static str>,
    scratch: tempfile::TempDir,
}

impl Fake {
    /// `None` (after saying why) if ffmpeg is missing and this run is allowed to skip.
    fn new() -> Option<Fake> {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let have_ffmpeg = Command::new(ffmpeg())
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !have_ffmpeg {
            assert!(
                std::env::var_os("NASTRO_REQUIRE_FFMPEG").is_none(),
                "NASTRO_REQUIRE_FFMPEG is set but ffmpeg is missing"
            );
            eprintln!("skipping: ffmpeg is not installed");
            return None;
        }
        let mut f = Fake {
            _guard: guard,
            set: Vec::new(),
            scratch: tempfile::tempdir().unwrap(),
        };
        f.env(
            "NASTRO_WHISPERX",
            env!("CARGO_BIN_EXE_nastro-fake-whisperx"),
        );
        let argv = f.scratch.path().join("argv.log");
        let envlog = f.scratch.path().join("env.log");
        f.env("FAKE_WX_ARGV_LOG", argv.to_str().unwrap());
        f.env("FAKE_WX_ENV_LOG", envlog.to_str().unwrap());
        Some(f)
    }

    fn env(&mut self, key: &'static str, value: &str) {
        std::env::set_var(key, value);
        self.set.push(key);
    }

    fn mode(&mut self, mode: &str) {
        self.env("FAKE_WX_MODE", mode);
    }

    fn argv(&self) -> Option<String> {
        std::fs::read_to_string(self.scratch.path().join("argv.log")).ok()
    }

    fn token_seen(&self) -> Option<String> {
        std::fs::read_to_string(self.scratch.path().join("env.log")).ok()
    }

    /// A recording directory holding a real (silent, 2 s) `audio.m4a`.
    fn recording(&self) -> PathBuf {
        let dir = self.scratch.path().join("rec");
        std::fs::create_dir_all(&dir).unwrap();
        let status = Command::new(ffmpeg())
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
        dir
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        for k in &self.set {
            std::env::remove_var(k);
        }
    }
}

fn opts() -> Options {
    Options {
        backend: Backend::WhisperX,
        model: "small".into(),
        lang: "en".into(),
        hf_token: Secret::new(TOKEN),
        threads: Some(2),
        ..Options::default()
    }
}

fn run(dir: &Path, o: &Options) -> (Result<nastro_transcribe::Outcome, Error>, Vec<Progress>) {
    let mut seen = Vec::new();
    let r = transcribe(dir, o, &CancelToken::new(), &mut |p| seen.push(p));
    (r, seen)
}

fn dir_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn whisperx_temp_dirs() -> usize {
    std::fs::read_dir(std::env::temp_dir())
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("nastro-whisperx-")
                })
                .count()
        })
        .unwrap_or(0)
}

// ----------------------------------------------------------------------------------------------- success

#[test]
fn a_diarized_run_writes_speaker_labelled_transcripts() {
    let Some(f) = Fake::new() else { return };
    let dir = f.recording();

    let (r, seen) = run(&dir, &opts());
    let out = r.unwrap();

    let txt = std::fs::read_to_string(dir.join("transcript.txt")).unwrap();
    assert_eq!(
        txt,
        "[SPEAKER_00] Good morning everyone. Let's begin the meeting. Café ñ 日本語\n[SPEAKER_01] Thanks. The release is on track.\n",
        "same-speaker phrases merge, and non-ASCII text survives"
    );
    let srt = std::fs::read_to_string(dir.join("transcript.srt")).unwrap();
    assert!(
        srt.contains("1\n00:00:00,150 --> 00:00:05,400\n[SPEAKER_00] Good morning"),
        "{srt}"
    );
    assert!(
        srt.contains("2\n00:00:07,200 --> 00:00:12,000\n[SPEAKER_01] Thanks."),
        "{srt}"
    );

    assert_eq!(out.speakers, 2);
    assert_eq!(out.language, "en");
    assert!(
        out.audio_seconds > 1.5 && out.audio_seconds < 2.6,
        "{}",
        out.audio_seconds
    );
    assert_eq!(
        dir_names(&dir),
        ["audio.m4a", "transcript.srt", "transcript.txt"],
        "no litter"
    );

    // Stages are reported in order, and WhisperX's own phases come through.
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
    assert_eq!(
        phases,
        [
            Phase::DetectingSpeech,
            Phase::Transcribing,
            Phase::Aligning,
            Phase::Diarizing
        ]
    );
    assert_eq!(seen.first(), Some(&Progress::Decoding));
    assert_eq!(seen.last(), Some(&Progress::Saving));
}

#[test]
fn the_token_reaches_whisperx_only_through_its_environment() {
    let Some(f) = Fake::new() else { return };
    let dir = f.recording();

    run(&dir, &opts()).0.unwrap();

    let argv = f.argv().expect("the fake was run");
    assert!(
        !argv.contains(TOKEN) && !argv.contains("hf_"),
        "the token is visible in argv:\n{argv}"
    );
    assert_eq!(f.token_seen().unwrap(), format!("HF_TOKEN={TOKEN}"));
    // And the command line is the documented one.
    for want in [
        "--diarize",
        "--language",
        "en",
        "--model",
        "small",
        "--device",
        "cpu",
        "--compute_type",
        "int8",
        "--threads",
        "2",
        "--output_format",
        "json",
    ] {
        assert!(argv.lines().any(|l| l == want), "missing {want:?}:\n{argv}");
    }
}

#[test]
fn the_speaker_count_is_passed_when_known() {
    let Some(f) = Fake::new() else { return };
    let dir = f.recording();
    let mut o = opts();
    o.speakers = Speakers::exactly(2);

    run(&dir, &o).0.unwrap();

    let argv = f.argv().unwrap();
    assert!(
        argv.contains("--min_speakers\n2") && argv.contains("--max_speakers\n2"),
        "{argv}"
    );
}

#[test]
fn without_diarization_no_token_is_needed_and_there_are_no_labels() {
    let Some(f) = Fake::new() else { return };
    let dir = f.recording();
    let mut o = opts();
    o.diarize = false;
    o.hf_token = None;

    let out = run(&dir, &o).0.unwrap();

    assert_eq!(out.speakers, 0);
    let txt = std::fs::read_to_string(dir.join("transcript.txt")).unwrap();
    assert!(
        !txt.contains("[SPEAKER") && txt.starts_with("Good morning everyone."),
        "{txt}"
    );
    assert!(!f.argv().unwrap().contains("--diarize"));
}

#[test]
fn an_auto_language_is_left_for_whisperx_to_detect() {
    let Some(f) = Fake::new() else { return };
    let dir = f.recording();
    let mut o = opts();
    o.lang = "auto".into();
    run(&dir, &o).0.unwrap();
    assert!(!f.argv().unwrap().contains("--language"));
}

// ------------------------------------------------------------------------------------ checks before running

#[test]
fn a_missing_token_is_reported_before_anything_runs() {
    let Some(f) = Fake::new() else { return };
    let dir = f.recording();
    let mut o = opts();
    o.hf_token = None;

    let (r, seen) = run(&dir, &o);

    assert!(matches!(r, Err(Error::HfTokenMissing)), "{r:?}");
    assert!(r
        .unwrap_err()
        .to_string()
        .contains("huggingface.co/settings/tokens"));
    assert!(f.argv().is_none(), "WhisperX must not have been started");
    assert!(seen.is_empty(), "no work was reported: {seen:?}");
    assert_eq!(dir_names(&dir), ["audio.m4a"]);
}

#[test]
fn a_missing_whisperx_explains_how_to_install_it() {
    let Some(mut f) = Fake::new() else { return };
    let dir = f.recording();
    f.env("NASTRO_WHISPERX", "this-path-does-not-exist");

    let (r, _) = run(&dir, &opts());

    let e = r.unwrap_err();
    assert!(matches!(e, Error::WhisperxMissing { .. }), "{e}");
    assert!(
        e.to_string().contains("--setup-whisperx")
            && e.to_string().contains("uv tool install whisperx")
    );
}

#[test]
fn a_file_path_model_is_rejected_for_whisperx_before_running() {
    let Some(f) = Fake::new() else { return };
    let dir = f.recording();
    let mut o = opts();
    o.model = r"C:\models\ggml-base.bin".into();
    let (r, _) = run(&dir, &o);
    assert!(matches!(r, Err(Error::WhisperxModel(_))), "{r:?}");
    assert!(f.argv().is_none());
}

#[test]
fn an_english_only_model_with_another_language_is_rejected_up_front() {
    let Some(f) = Fake::new() else { return };
    let dir = f.recording();
    let mut o = opts();
    o.model = "small.en".into();
    o.lang = "de".into();
    assert!(matches!(
        run(&dir, &o).0,
        Err(Error::EnglishOnlyModel { .. })
    ));
    assert!(f.argv().is_none());
}

// --------------------------------------------------------------------------------------------- failures

/// Every failure leaves an existing transcript exactly as it was, and no temporary files behind.
fn assert_failure(mode: &str, check: impl Fn(&Error)) {
    let Some(mut f) = Fake::new() else { return };
    f.mode(mode);
    let dir = f.recording();
    std::fs::write(dir.join("transcript.txt"), "previous\n").unwrap();
    let mut o = opts();
    o.overwrite = true;
    let temp_before = whisperx_temp_dirs();

    let (r, _) = run(&dir, &o);

    let e = r.expect_err(mode);
    check(&e);
    assert!(
        !e.to_string().contains(TOKEN),
        "{mode}: the token leaked into the error: {e}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("transcript.txt")).unwrap(),
        "previous\n",
        "{mode}"
    );
    assert_eq!(
        dir_names(&dir),
        ["audio.m4a", "transcript.txt"],
        "{mode}: litter"
    );
    assert_eq!(
        whisperx_temp_dirs(),
        temp_before,
        "{mode}: the temporary folder was not removed"
    );
}

#[test]
fn a_gated_model_says_which_terms_to_accept() {
    assert_failure("gated", |e| match e {
        Error::DiarizationAccess { repo } => {
            assert_eq!(repo, "pyannote/speaker-diarization-community-1")
        }
        e => panic!("wrong error: {e}"),
    });
}

#[test]
fn a_rejected_token_is_reported() {
    assert_failure("token", |e| {
        assert!(matches!(e, Error::HfTokenRejected), "{e}")
    });
}

#[test]
fn missing_nltk_data_points_at_the_setup_command() {
    assert_failure("punkt", |e| {
        assert!(matches!(e, Error::WhisperxSetup(_)), "{e}");
        assert!(e.to_string().contains("--setup-whisperx"));
    });
}

#[test]
fn an_unexplained_failure_is_shown_with_the_token_scrubbed() {
    assert_failure("fail", |e| match e {
        Error::WhisperxFailed { detail, .. } => {
            assert!(detail.contains("something exploded"), "{detail}");
            assert!(
                detail.contains("***"),
                "the token should have been replaced: {detail}"
            );
        }
        e => panic!("wrong error: {e}"),
    });
}

#[test]
fn unreadable_or_missing_results_are_errors_not_empty_transcripts() {
    assert_failure("garbage", |e| {
        assert!(matches!(e, Error::WhisperxOutput(_)), "{e}")
    });
    assert_failure("nooutput", |e| {
        assert!(matches!(e, Error::WhisperxOutput(_)), "{e}")
    });
}

// -------------------------------------------------------------------------------------------- cancelling

#[test]
fn cancelling_stops_whisperx_promptly_and_leaves_everything_as_it_was() {
    let Some(mut f) = Fake::new() else { return };
    f.mode("hang");
    let dir = f.recording();
    std::fs::write(dir.join("transcript.txt"), "previous\n").unwrap();
    let mut o = opts();
    o.overwrite = true;
    let temp_before = whisperx_temp_dirs();

    let cancel = CancelToken::new();
    let c = cancel.clone();
    let mut cancelled_at = None;
    let r = transcribe(&dir, &o, &cancel, &mut |p| {
        if matches!(p, Progress::Phase(Phase::Transcribing)) && cancelled_at.is_none() {
            cancelled_at = Some(Instant::now());
            c.cancel();
        }
    });

    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    let took = cancelled_at
        .expect("it reached the transcription phase")
        .elapsed();
    assert!(took < Duration::from_secs(5), "cancel took {took:?}");
    assert_eq!(
        std::fs::read_to_string(dir.join("transcript.txt")).unwrap(),
        "previous\n"
    );
    assert_eq!(dir_names(&dir), ["audio.m4a", "transcript.txt"]);
    assert_eq!(
        whisperx_temp_dirs(),
        temp_before,
        "temporary folder removed"
    );
}

fn process_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.process(Pid::from_u32(pid)).is_some()
}

/// On Windows the installed `whisperx.exe` is only a launcher around Python, so killing just the launcher would
/// leave the real work running. The stand-in's "tree" mode starts a descendant, like that.
#[test]
fn cancelling_kills_the_whole_process_tree_not_just_the_launcher() {
    let Some(mut f) = Fake::new() else { return };
    f.mode("tree");
    let pidfile = f.scratch.path().join("child.pid");
    f.env("FAKE_WX_PIDFILE", pidfile.to_str().unwrap());
    let dir = f.recording();

    let cancel = CancelToken::new();
    let c = cancel.clone();
    let r = transcribe(&dir, &opts(), &cancel, &mut |p| {
        if matches!(p, Progress::Phase(Phase::Transcribing)) {
            c.cancel();
        }
    });
    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");

    let pid: u32 = std::fs::read_to_string(&pidfile)
        .expect("the stand-in recorded its descendant's pid")
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let alive = process_alive(pid);
    if alive {
        // Don't leave it running if the assertion below is about to fail.
        #[cfg(windows)]
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status();
        #[cfg(unix)]
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
    assert!(!alive, "the descendant process {pid} was left running");
}

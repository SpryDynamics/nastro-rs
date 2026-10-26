//! On-device transcription of nastro recordings.
//!
//! A recording directory holds `audio.m4a`; [`transcribe`] decodes it with `ffmpeg` (a run-time dependency,
//! never linked), recognises speech with an embedded whisper.cpp, and writes `transcript.txt` and
//! `transcript.srt` next to it. Models are downloaded on first use, from a pinned source, and verified
//! against a built-in SHA-256 before they are used (see [`models`]).
//!
//! Nothing leaves the machine except that one model download.

pub mod audio;
pub mod engine;
pub mod hf;
pub mod models;
pub mod output;
pub mod whisperx;

mod error;
mod model_table;
mod process;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use error::{Error, Result};
pub use model_table::{ModelInfo, MODELS};
pub use output::Segment;

/// The audio file inside a recording directory (written by the recorder).
pub const AUDIO_FILE: &str = "audio.m4a";

/// A cloneable flag used to stop a transcription (or model download) from another thread.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub(crate) fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

/// Which engine does the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// The built-in whisper.cpp: fast, nothing to install, no speaker labels.
    #[default]
    Whisper,
    /// The external WhisperX tool: slower and heavier, but labels who is speaking (`[SPEAKER_00]`).
    WhisperX,
}

impl Backend {
    /// Parses the `transcriber` config value. `whisper-cli` is the name the original nastro used for the same
    /// default engine.
    pub fn from_name(name: &str) -> Result<Backend> {
        match name.trim().to_ascii_lowercase().as_str() {
            "" | "whisper" | "whisper-cli" => Ok(Backend::Whisper),
            "whisperx" => Ok(Backend::WhisperX),
            _ => Err(Error::UnknownTranscriber(name.to_string())),
        }
    }
}

/// A credential that must never appear in a log, a panic message or a `Debug` print.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// `None` for an empty (or whitespace-only) value.
    pub fn new(value: impl Into<String>) -> Option<Secret> {
        let v = value.into();
        let v = v.trim();
        (!v.is_empty()).then(|| Secret(v.to_string()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// How many people are in the recording, if known. Telling the identification step helps it a lot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Speakers {
    pub min: Option<u32>,
    pub max: Option<u32>,
}

impl Speakers {
    /// Exactly `n` speakers.
    pub fn exactly(n: u32) -> Speakers {
        Speakers {
            min: Some(n),
            max: Some(n),
        }
    }
}

/// What to transcribe with.
#[derive(Debug, Clone)]
pub struct Options {
    /// Model name from [`MODELS`] (e.g. `large-v3-turbo`) or the path of a ggml `.bin` file. With
    /// [`Backend::WhisperX`] it must be a plain model name: WhisperX fetches its own copy.
    pub model: String,
    /// A language code (`en`, `de`, ...) or `auto` to detect it.
    pub lang: String,
    /// CPU threads for recognition; `None` picks a sensible number for this machine.
    pub threads: Option<usize>,
    /// Replace an existing transcript. Without it, an existing one is an error.
    pub overwrite: bool,
    pub backend: Backend,
    /// Hugging Face token for the gated speaker models ([`Backend::WhisperX`] only).
    pub hf_token: Option<Secret>,
    /// Number of speakers, if known ([`Backend::WhisperX`] only).
    pub speakers: Speakers,
    /// With [`Backend::WhisperX`], label the speakers. Always on in normal use; the switch exists so the rest of
    /// the WhisperX pipeline can be exercised without a Hugging Face account.
    pub diarize: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            model: nastro_config::DEFAULT_WHISPER_MODEL.to_string(),
            lang: nastro_config::DEFAULT_LANG.to_string(),
            threads: None,
            overwrite: false,
            backend: Backend::Whisper,
            hf_token: None,
            speakers: Speakers::default(),
            diarize: true,
        }
    }
}

impl Options {
    /// Options from the config file. (An unknown `transcriber` falls back to the built-in engine here; callers
    /// report it with [`check_transcriber`] first.)
    pub fn from_config(config: &nastro_config::Config) -> Self {
        Options {
            model: config.whisper_model.clone(),
            lang: config.lang.clone(),
            backend: Backend::from_name(&config.transcriber).unwrap_or_default(),
            hf_token: Secret::new(config.resolved_hf_token()),
            ..Options::default()
        }
    }
}

/// The stages of a WhisperX run. It reports no percentage, only which step it has reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Checking / installing what WhisperX needs.
    Preparing,
    DetectingSpeech,
    Transcribing,
    Aligning,
    Diarizing,
}

impl Phase {
    /// A short phrase for a status line.
    pub fn label(self) -> &'static str {
        match self {
            Phase::Preparing => "preparing speaker tools",
            Phase::DetectingSpeech => "detecting speech",
            Phase::Transcribing => "transcribing",
            Phase::Aligning => "aligning words",
            Phase::Diarizing => "identifying speakers",
        }
    }
}

/// Coarse progress, in the order the stages happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Downloading the model (first use only); bytes so far / total.
    DownloadingModel { done: u64, total: u64 },
    /// Reading `audio.m4a`.
    Decoding,
    /// Recognising speech; 0..=100.
    Transcribing { percent: u8 },
    /// A WhisperX stage (it has no percentage).
    Phase(Phase),
    /// Writing the transcript files.
    Saving,
}
/// The result of a successful run.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub dir: PathBuf,
    /// Number of non-empty segments written (0 when no speech was found).
    pub segments: usize,
    /// Language used (the detected one for `auto`).
    pub language: String,
    pub audio_seconds: f64,
    /// Distinct speakers found (0 when the transcript has no speaker labels).
    pub speakers: usize,
    /// Time spent decoding, recognising and saving (not downloading the model).
    pub elapsed: Duration,
}

/// Logical CPUs available to this process.
fn logical_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// The most threads it is safe to run recognition on.
///
/// whisper.cpp's worker threads spin-wait on each other, so once they use every hardware thread, any other
/// work on the machine (even idle background noise) can stall one of them and make all the others burn their
/// time slices waiting: the run does not just stop speeding up, it collapses. Measured, tiny model, 220 s of audio:
///
/// | machine                | threads | time     |
/// |------------------------|---------|----------|
/// | 16 cores / 32 threads  | 16      | 8.7 s    |
/// |                        | 24      | 12.4 s   |
/// |                        | 31      | > 90 s   |
/// | 6 vCPUs (WSL2)         | 5       | 16.0 s   |
/// |                        | 6       | 27.1 s   |
/// |                        | 8       | 12 min   |
///
/// So: the number of physical cores when SMT/hyper-threading makes that smaller than the CPU count, otherwise one
/// fewer than the CPU count (leaving a CPU for everything else).
pub fn max_threads() -> usize {
    let logical = logical_cpus();
    match sysinfo::System::new().physical_core_count() {
        Some(physical) if physical > 0 && physical < logical => physical,
        _ => logical.saturating_sub(1).max(1),
    }
}

/// Threads to use when the caller doesn't say: as many as are safe ([`max_threads`]), but at most 16 (recognition
/// scales poorly beyond that).
pub fn default_threads() -> usize {
    max_threads().min(16)
}

/// The thread count that will actually be used for a request: never more than [`max_threads`], however much is
/// asked for.
pub fn effective_threads(requested: Option<usize>) -> usize {
    requested
        .unwrap_or_else(default_threads)
        .clamp(1, max_threads())
}

/// Checks the `transcriber` config value: `whisper` (alias `whisper-cli`) or `whisperx`.
pub fn check_transcriber(name: &str) -> Result<Backend> {
    Backend::from_name(name)
}
/// Picks the recording matching `id_or_last` from `records` (as returned by `nastro_records::scan`, newest
/// first): `last` is the newest, anything else must equal a directory name exactly.
pub fn resolve<'a>(
    records: &'a [nastro_records::Record],
    id_or_last: &str,
) -> Result<&'a nastro_records::Record> {
    let found = if id_or_last == "last" {
        records.iter().max_by(|a, b| a.dir_name.cmp(&b.dir_name))
    } else {
        records.iter().find(|r| r.dir_name == id_or_last)
    };
    found.ok_or_else(|| Error::RecordNotFound(id_or_last.to_string()))
}

/// `true` if `target_dir_name` is the recording currently being written: the recorder holds a global lock
/// (`<output_dir>/.lock`, containing its PID) while it runs, and that recording is always the newest.
/// `is_alive` is injected so this can be tested without real processes.
pub fn recording_in_progress(
    output_dir: &Path,
    newest_dir_name: Option<&str>,
    target_dir_name: &str,
    is_alive: &dyn Fn(u32) -> bool,
) -> bool {
    if newest_dir_name != Some(target_dir_name) {
        return false;
    }
    std::fs::read_to_string(output_dir.join(".lock"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(is_alive)
}

/// Transcribes the recording in `dir`.
///
/// With [`Backend::Whisper`]: check for an existing transcript, download the model if needed, decode the audio,
/// recognise, save. With [`Backend::WhisperX`]: the same, but WhisperX does the recognition and labels the speakers.
/// Cancelling at any stage returns [`Error::Cancelled`] and leaves any existing transcript untouched, as does
/// any other failure: the new transcript replaces the old one only once it is complete.
pub fn transcribe(
    dir: &Path,
    options: &Options,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<Outcome> {
    let audio = dir.join(AUDIO_FILE);
    if !audio.is_file() {
        return Err(Error::NoAudio(dir.to_path_buf()));
    }
    let transcript = dir.join(output::TXT_FILE);
    if transcript.exists() && !options.overwrite {
        return Err(Error::AlreadyTranscribed(transcript));
    }

    match options.backend {
        Backend::Whisper => transcribe_whisper(dir, &audio, options, cancel, on_progress),
        Backend::WhisperX => transcribe_whisperx(dir, &audio, options, cancel, on_progress),
    }
}

fn transcribe_whisper(
    dir: &Path,
    audio: &Path,
    options: &Options,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<Outcome> {
    // Cheap checks first, so a typo or an unsupported CPU fails before a multi-gigabyte download.
    if !engine::cpu_supported() {
        return Err(Error::UnsupportedCpu);
    }
    let language = engine::resolve_language(&options.model, &options.lang)?;

    let model_path = models::ensure(&options.model, cancel, &mut |done, total| {
        on_progress(Progress::DownloadingModel { done, total });
    })?;

    cancel.check()?;
    // Time the work itself: a first-use model download would otherwise be counted as "transcribing".
    let started = Instant::now();
    on_progress(Progress::Decoding);
    let samples = audio::decode(audio, cancel)?;
    let audio_seconds = samples.len() as f64 / f64::from(audio::SAMPLE_RATE);

    on_progress(Progress::Transcribing { percent: 0 });
    let threads = effective_threads(options.threads);
    let recognition = engine::recognize(
        &model_path,
        &language,
        threads,
        &samples,
        cancel,
        &mut |percent| {
            on_progress(Progress::Transcribing { percent });
        },
    )?;
    drop(samples);

    cancel.check()?;
    on_progress(Progress::Saving);
    output::write_all(dir, &recognition.segments)?;

    Ok(Outcome {
        dir: dir.to_path_buf(),
        segments: recognition.segments.len(),
        language: recognition.language,
        audio_seconds,
        speakers: 0,
        elapsed: started.elapsed(),
    })
}

fn transcribe_whisperx(
    dir: &Path,
    audio: &Path,
    options: &Options,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<Outcome> {
    // Everything that can be checked cheaply is checked first, so a missing tool or token fails in a moment rather
    // than after minutes of work.
    let language = engine::resolve_language(&options.model, &options.lang)?;
    whisperx::model_name(&options.model)?;
    let exe = whisperx::ready(options, on_progress)?;

    cancel.check()?;
    let started = Instant::now();
    let result = whisperx::run(&exe, audio, options, &language, cancel, on_progress)?;

    cancel.check()?;
    on_progress(Progress::Saving);
    output::write_all(dir, &result.segments)?;

    let speakers = result
        .segments
        .iter()
        .filter_map(|s| s.speaker.as_deref())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    Ok(Outcome {
        dir: dir.to_path_buf(),
        segments: result
            .segments
            .iter()
            .filter(|s| !s.text.trim().is_empty())
            .count(),
        language: result.language,
        audio_seconds: result.audio_seconds,
        speakers,
        elapsed: started.elapsed(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use nastro_records::Record;

    fn rec(name: &str) -> Record {
        Record {
            dir_name: name.to_string(),
            path: PathBuf::from(name),
            parsed: nastro_records::parse_dir_name("2026-01-01-0900").expect("valid name"),
            size_bytes: 0,
            duration_seconds: None,
            has_transcript: false,
        }
    }

    #[test]
    fn last_is_the_newest_by_name_whatever_the_order() {
        let recs = vec![
            rec("2026-01-01-0900-a"),
            rec("2026-03-01-0900-c"),
            rec("2026-02-01-0900-b"),
        ];
        assert_eq!(
            resolve(&recs, "last").unwrap().dir_name,
            "2026-03-01-0900-c"
        );
    }

    #[test]
    fn an_id_must_match_exactly() {
        let recs = vec![rec("2026-01-01-0900-a")];
        assert!(resolve(&recs, "2026-01-01-0900-a").is_ok());
        assert!(matches!(
            resolve(&recs, "2026-01-01"),
            Err(Error::RecordNotFound(_))
        ));
        assert!(matches!(
            resolve(&[], "last"),
            Err(Error::RecordNotFound(_))
        ));
    }

    #[test]
    fn only_the_newest_recording_can_be_in_progress_and_only_with_a_live_lock() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".lock"), "4242").unwrap();
        let alive = |pid: u32| pid == 4242;
        let dead = |_: u32| false;

        assert!(recording_in_progress(
            dir.path(),
            Some("new"),
            "new",
            &alive
        ));
        assert!(
            !recording_in_progress(dir.path(), Some("new"), "old", &alive),
            "older recordings are finished"
        );
        assert!(
            !recording_in_progress(dir.path(), Some("new"), "new", &dead),
            "stale lock"
        );
        assert!(!recording_in_progress(dir.path(), None, "new", &alive));

        std::fs::write(dir.path().join(".lock"), "garbage").unwrap();
        assert!(!recording_in_progress(
            dir.path(),
            Some("new"),
            "new",
            &alive
        ));
        std::fs::remove_file(dir.path().join(".lock")).unwrap();
        assert!(
            !recording_in_progress(dir.path(), Some("new"), "new", &alive),
            "no lock at all"
        );
    }

    #[test]
    fn missing_audio_and_existing_transcripts_are_reported_before_any_work() {
        let dir = tempfile::tempdir().unwrap();
        let opts = Options {
            model: "tiny.en".into(),
            lang: "auto".into(),
            ..Options::default()
        };
        let mut calls = 0;

        let e =
            transcribe(dir.path(), &opts, &CancelToken::new(), &mut |_| calls += 1).unwrap_err();
        assert!(matches!(e, Error::NoAudio(_)), "{e}");

        std::fs::write(dir.path().join(AUDIO_FILE), b"x").unwrap();
        std::fs::write(dir.path().join(output::TXT_FILE), "old").unwrap();
        let e =
            transcribe(dir.path(), &opts, &CancelToken::new(), &mut |_| calls += 1).unwrap_err();
        assert!(matches!(e, Error::AlreadyTranscribed(_)), "{e}");
        assert_eq!(
            calls, 0,
            "no progress was reported for work that never started"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(output::TXT_FILE)).unwrap(),
            "old"
        );
    }

    #[test]
    fn a_bad_language_fails_before_any_download() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(AUDIO_FILE), b"x").unwrap();
        let opts = Options {
            model: "tiny".into(),
            lang: "klingon".into(),
            ..Options::default()
        };
        let e = transcribe(dir.path(), &opts, &CancelToken::new(), &mut |_| {
            panic!("no work expected")
        })
        .unwrap_err();
        assert!(matches!(e, Error::UnknownLanguage(_)), "{e}");
    }

    #[test]
    fn thread_counts_are_sane_and_never_oversubscribe() {
        let cpus = logical_cpus();
        let max = max_threads();
        let d = default_threads();
        assert!(
            max >= 1 && (max < cpus || cpus == 1),
            "must leave a CPU/hyper-thread free: {max} of {cpus}"
        );
        assert!((1..=16).contains(&d) && d <= max, "default {d}, max {max}");
        assert_eq!(effective_threads(None), d);
        assert_eq!(effective_threads(Some(0)), 1);
        assert_eq!(effective_threads(Some(1)), 1);
        assert_eq!(
            effective_threads(Some(10_000)),
            max,
            "a huge request is clamped to the safe maximum"
        );
    }

    #[test]
    fn transcriber_names() {
        for ok in ["whisper", "whisper-cli", "", " Whisper "] {
            assert_eq!(check_transcriber(ok).unwrap(), Backend::Whisper, "{ok:?}");
        }
        for x in ["whisperx", " WhisperX "] {
            assert_eq!(check_transcriber(x).unwrap(), Backend::WhisperX, "{x:?}");
        }
        assert!(matches!(
            check_transcriber("deepgram"),
            Err(Error::UnknownTranscriber(_))
        ));
    }

    #[test]
    fn cancel_token_is_shared_between_clones() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled());
        assert!(matches!(b.check(), Err(Error::Cancelled)));
    }
}

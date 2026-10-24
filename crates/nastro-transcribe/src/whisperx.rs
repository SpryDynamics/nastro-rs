//! Speaker identification by running [WhisperX](https://github.com/m-bain/whisperX) as a separate program.
//!
//! WhisperX is Python (PyTorch and pyannote), several gigabytes, and its speaker models are gated behind a free
//! Hugging Face account, so it is *not* bundled: nastro finds the `whisperx` command, runs it on a 16 kHz copy of
//! the recording, reads the JSON it writes, and turns that into the same `transcript.txt` / `transcript.srt`
//! as the built-in engine, with `[SPEAKER_00]` labels.
//!
//! Things this module is careful about:
//!
//! * The Hugging Face token reaches WhisperX only through its **environment** (never argv, which any local user
//!   can read with `ps`), and is scrubbed from anything echoed back in an error.
//! * WhisperX reports no percentage, only which stage it is in, so that is what is shown (see [`Phase`]).
//! * Cancelling kills WhisperX *and everything it started*; on Windows the installed `whisperx.exe` is just a
//!   launcher around a Python child.
//! * Nothing is written next to the recording until WhisperX has succeeded and its output has parsed.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde::Deserialize;

use crate::output::Segment;
use crate::{CancelToken, Error, Options, Phase, Progress, Result, Secret, Speakers};

/// Set to the full path of a `whisperx` executable to use that instead of searching (also how the tests run a
/// stand-in).
pub const ENV_OVERRIDE: &str = "NASTRO_WHISPERX";

/// What WhisperX produced.
#[derive(Debug, Clone)]
pub struct Output {
    pub segments: Vec<Segment>,
    /// The language WhisperX worked in.
    pub language: String,
    pub audio_seconds: f64,
}

// ------------------------------------------------------------------------------------------ finding programs

/// Looks for `name` on `PATH` and in `~/.local/bin`, where `uv tool install` and `pipx` put their launchers and
/// which is often not on `PATH` yet. On Windows only `.exe` launchers are accepted (running `.cmd`/`.bat` files
/// has argument-escaping pitfalls and is never needed here).
pub(crate) fn find_program(name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let mut dirs: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    if let Some(dir) = std::env::var_os("UV_TOOL_BIN_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(base) = directories::BaseDirs::new() {
        dirs.push(base.home_dir().join(".local").join("bin"));
    }
    first_in(&file, &dirs)
}

/// The first of `dirs` that contains a file called `file`.
fn first_in(file: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(file)).find(|p| p.is_file())
}

/// The `whisperx` command: `$NASTRO_WHISPERX` if set (and it must exist), otherwise a search.
pub fn locate() -> Option<PathBuf> {
    match std::env::var_os(ENV_OVERRIDE) {
        Some(p) => Some(PathBuf::from(p)).filter(|p| p.is_file()),
        None => find_program("whisperx"),
    }
}

/// The `uv` command, used to install WhisperX.
pub fn locate_uv() -> Option<PathBuf> {
    find_program("uv")
}

// ------------------------------------------------------------------------------------------ pure helpers

/// WhisperX takes a faster-whisper model *name*. These are the ones that exist.
const MODELS: &[&str] = &[
    "tiny",
    "tiny.en",
    "base",
    "base.en",
    "small",
    "small.en",
    "medium",
    "medium.en",
    "large-v1",
    "large-v2",
    "large-v3",
    "large-v3-turbo",
];

/// Maps the `whisper_model` setting to a WhisperX model name. The ggml quantised variants (`base-q5_1`, ...) map
/// to the full model (WhisperX has its own quantisation, `--compute_type int8`); a file path cannot be used.
pub fn model_name(model: &str) -> Result<String> {
    if crate::models::is_path(model) {
        return Err(Error::WhisperxModel(model.to_string()));
    }
    let base = ["-q5_0", "-q5_1", "-q8_0"]
        .iter()
        .find_map(|q| model.strip_suffix(q))
        .unwrap_or(model);
    if MODELS.contains(&base) {
        Ok(base.to_string())
    } else {
        Err(Error::WhisperxModel(model.to_string()))
    }
}

/// The command line for WhisperX. The Hugging Face token is deliberately absent (see the module docs). `language`
/// `auto` (or empty) leaves WhisperX to detect it.
pub fn args(
    wav: &Path,
    language: &str,
    model: &str,
    out_dir: &Path,
    threads: usize,
    diarize: bool,
    speakers: Speakers,
) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec![wav.as_os_str().to_os_string()];
    let mut push = |s: &str| a.push(OsString::from(s));
    if diarize {
        push("--diarize");
    }
    if !language.is_empty() && language != "auto" {
        push("--language");
        push(language);
    }
    push("--model");
    push(model);
    push("--device");
    push("cpu");
    push("--compute_type");
    push("int8");
    push("--threads");
    push(&threads.to_string());
    if diarize {
        if let Some(n) = speakers.min {
            push("--min_speakers");
            push(&n.to_string());
        }
        if let Some(n) = speakers.max {
            push("--max_speakers");
            push(&n.to_string());
        }
    }
    push("--output_dir");
    a.push(out_dir.as_os_str().to_os_string());
    let mut push = |s: &str| a.push(OsString::from(s));
    push("--output_format");
    push("json");
    a
}

#[derive(Deserialize)]
struct RawOutput {
    #[serde(default)]
    segments: Vec<RawSegment>,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Deserialize)]
struct RawSegment {
    #[serde(default)]
    start: f64,
    #[serde(default)]
    end: f64,
    #[serde(default)]
    text: String,
    #[serde(default)]
    speaker: Option<String>,
}

fn millis(seconds: f64) -> i64 {
    if seconds.is_finite() {
        (seconds * 1000.0).round().max(0.0) as i64
    } else {
        0
    }
}

/// Parses the JSON WhisperX writes (`--output_format json`): a flat `segments` array, each with `start` / `end`
/// in seconds, `text`, and `speaker` when speakers were identified. (Per-word timings are present but not used.)
/// A segment with no speaker keeps `None`.
pub fn parse_output(json: &[u8]) -> Result<(Vec<Segment>, Option<String>)> {
    let raw: RawOutput = serde_json::from_slice(json)
        .map_err(|e| Error::WhisperxOutput(format!("not the JSON that was expected: {e}")))?;
    let segments = raw
        .segments
        .into_iter()
        .map(|s| Segment {
            start_ms: millis(s.start),
            end_ms: millis(s.end),
            text: s.text.trim().to_string(),
            speaker: s.speaker.filter(|x| !x.trim().is_empty()),
        })
        .collect();
    Ok((segments, raw.language))
}

/// Which stage a line of WhisperX's log announces, if any.
pub fn phase_of(line: &str) -> Option<Phase> {
    const MARKERS: &[(&str, Phase)] = &[
        (
            "Performing voice activity detection",
            Phase::DetectingSpeech,
        ),
        ("Performing transcription", Phase::Transcribing),
        ("Performing alignment", Phase::Aligning),
        ("Performing diarization", Phase::Diarizing),
    ];
    MARKERS
        .iter()
        .find(|(m, _)| line.contains(m))
        .map(|(_, p)| *p)
}

/// Removes the token (and anything shaped like a Hugging Face token) from text that is about to be shown.
pub fn scrub(text: &str, token: Option<&Secret>) -> String {
    let mut out = match token {
        Some(t) if t.expose().len() >= 4 => text.replace(t.expose(), "***"),
        _ => text.to_string(),
    };
    // Anything of the form hf_ followed by 20+ letters/digits is a token, whether or not it is ours.
    let mut result = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(i) = rest.find("hf_") {
        let after = &rest[i + 3..];
        let run = after.bytes().take_while(u8::is_ascii_alphanumeric).count();
        result.push_str(&rest[..i]);
        if run >= 20 {
            result.push_str("hf_***");
            rest = &after[run..];
        } else {
            result.push_str("hf_");
            rest = after;
        }
    }
    result.push_str(rest);
    out = result;
    out
}

/// Lines that are noise when explaining a failure (warnings that appear on every run, even successful ones).
fn is_noise(line: &str) -> bool {
    let l = line.trim();
    l.is_empty()
        || l.contains("torchcodec")
        || l.contains("libtorchcodec")
        || l.starts_with("FFmpeg version")
        || l.starts_with("[start of")
        || l.starts_with("[end of")
        || l.contains("UserWarning")
        || l.starts_with("warnings.warn")
        || l.starts_with("Lightning automatically upgraded")
        || l.starts_with("* ")
}

fn extract_gated_repo(text: &str) -> String {
    const DEFAULT: &str = "pyannote/speaker-diarization-community-1";
    // "Access to model pyannote/speaker-diarization-community-1 is restricted."
    if let Some(i) = text.find("Access to model ") {
        let rest = &text[i + "Access to model ".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
            .collect();
        if name.contains('/') {
            return name;
        }
    }
    DEFAULT.to_string()
}

/// Turns the tail of a failed run's output into the most helpful error.
pub fn classify_failure(tail: &[String], status: &str, token: Option<&Secret>) -> Error {
    let text = tail.join("\n");
    let lower = text.to_ascii_lowercase();

    if lower.contains("punkt_tab") {
        return Error::WhisperxSetup(
            "the sentence-splitting data (NLTK `punkt_tab`) is not installed. Run `nastro transcribe --setup-whisperx` to fetch it"
                .into(),
        );
    }
    // Hugging Face answers 401 when the token is missing or invalid, and 403 when a valid token has not been granted
    // the gated model. They need different fixes, so tell them apart (a real run with a bad token produces
    // "GatedRepoError: 401 Client Error ... Cannot access gated repo").
    let gated = lower.contains("gatedrepoerror")
        || lower.contains("gated repo")
        || lower.contains("is restricted");
    if gated && lower.contains("401 client error") {
        return Error::HfTokenRejected;
    }
    if gated {
        return Error::DiarizationAccess {
            repo: extract_gated_repo(&text),
        };
    }
    if lower.contains("invalid user token")
        || lower.contains("invalid credentials")
        || lower.contains("token is invalid")
        || (lower.contains("401") && (lower.contains("huggingface") || lower.contains("hub")))
    {
        return Error::HfTokenRejected;
    }
    if let Some(line) = tail.iter().find(|l| l.contains("No module named")) {
        return Error::WhisperxSetup(scrub(line.trim(), token));
    }

    let detail: Vec<String> = tail
        .iter()
        .filter(|l| !is_noise(l))
        .map(|l| scrub(l.trim(), token))
        .collect();
    let keep = detail.len().saturating_sub(10);
    Error::WhisperxFailed {
        status: status.to_string(),
        detail: detail[keep..].join("\n"),
    }
}

// ------------------------------------------------------------------------------------------ readiness / setup

/// Reads lines (split on `\n` and `\r`, as progress bars use the latter) from a pipe and forwards them.
fn pump(mut pipe: impl Read + Send + 'static, tx: mpsc::Sender<String>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut line: Vec<u8> = Vec::new();
        let send = |line: &mut Vec<u8>, tx: &mpsc::Sender<String>| {
            if !line.is_empty() {
                let _ = tx.send(String::from_utf8_lossy(line).into_owned());
                line.clear();
            }
        };
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &b in &buf[..n] {
                        if b == b'\n' || b == b'\r' {
                            send(&mut line, &tx);
                        } else if line.len() < 2048 {
                            line.push(b);
                        }
                    }
                }
            }
        }
        send(&mut line, &tx);
    });
}

/// The Python inside the `uv`-managed WhisperX environment (used for the one-off NLTK data step), if WhisperX was
/// installed that way.
fn tool_python(uv: &Path) -> Option<PathBuf> {
    let out = Command::new(uv).args(["tool", "dir"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let env = dir.join("whisperx");
    let python = if cfg!(windows) {
        env.join("Scripts").join("python.exe")
    } else {
        env.join("bin").join("python")
    };
    python.is_file().then_some(python)
}

fn run_python(python: &Path, code: &str) -> std::io::Result<bool> {
    let mut cmd = Command::new(python);
    cmd.args(["-c", code])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::process::configure(&mut cmd);
    Ok(cmd.status()?.success())
}

/// WhisperX needs NLTK's `punkt_tab` sentence data to align words, and a fresh install does not have it (the run
/// dies with a `LookupError` once it reaches alignment, minutes in). Check up front and fetch it if missing.
///
/// Only possible when WhisperX is `uv`-managed; otherwise a missing file is reported by [`classify_failure`].
fn ensure_nltk(on_progress: &mut dyn FnMut(Progress)) -> Result<()> {
    let Some(python) = locate_uv().and_then(|uv| tool_python(&uv)) else {
        return Ok(());
    };
    let ready = run_python(
        &python,
        "import nltk; nltk.data.find('tokenizers/punkt_tab/english/')",
    )
    .map_err(|e| Error::io("starting WhisperX's Python", e))?;
    if ready {
        return Ok(());
    }
    on_progress(Progress::Phase(Phase::Preparing));
    let fetched = run_python(
        &python,
        "import nltk, sys; sys.exit(0 if nltk.download('punkt_tab', quiet=True) else 1)",
    )
    .map_err(|e| Error::io("starting WhisperX's Python", e))?;
    if fetched {
        Ok(())
    } else {
        Err(Error::WhisperxSetup(
            "could not download NLTK's `punkt_tab` data (is this computer online?)".into(),
        ))
    }
}

/// Everything that can be checked before the (long) run starts: WhisperX exists, there is a token, and its
/// one-off data is in place. Returns the executable to run.
pub fn ready(options: &Options, on_progress: &mut dyn FnMut(Progress)) -> Result<PathBuf> {
    let exe = locate().ok_or_else(|| Error::WhisperxMissing {
        searched: match std::env::var_os(ENV_OVERRIDE) {
            Some(p) => p.to_string_lossy().into_owned(),
            None => "PATH and ~/.local/bin".to_string(),
        },
    })?;
    if options.diarize && options.hf_token.is_none() {
        return Err(Error::HfTokenMissing);
    }
    // The stand-in used by tests has no Python to check.
    if std::env::var_os(ENV_OVERRIDE).is_none() {
        ensure_nltk(on_progress)?;
    }
    Ok(exe)
}

/// Installs WhisperX with `uv` (unless it is already there) and fetches its one-off data. `on_line` receives the
/// installer's output as it runs. This is the only place nastro installs anything, and only when asked to.
pub fn install(on_line: &mut dyn FnMut(&str), cancel: &CancelToken) -> Result<()> {
    if let Some(existing) = locate() {
        on_line(&format!(
            "WhisperX is already installed: {}",
            existing.display()
        ));
    } else {
        install_with_uv(on_line, cancel)?;
    }
    on_line("Checking WhisperX's sentence-splitting data...");
    ensure_nltk(&mut |_| {})?;
    Ok(())
}

fn install_with_uv(on_line: &mut dyn FnMut(&str), cancel: &CancelToken) -> Result<()> {
    let uv = locate_uv().ok_or_else(|| {
        Error::WhisperxSetup(
            "the `uv` tool is needed to install WhisperX and was not found. Install it from https://docs.astral.sh/uv/ \
             (Windows: `winget install astral-sh.uv`), then run this again"
                .into(),
        )
    })?;

    // Pinned to a Python that WhisperX supports; uv fetches it if this computer has none.
    let mut cmd = Command::new(&uv);
    cmd.args(["tool", "install", "whisperx", "--python", "3.12"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::configure(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| Error::io("starting uv", e))?;

    let (tx, rx) = mpsc::channel();
    pump(child.stdout.take().expect("piped"), tx.clone());
    pump(child.stderr.take().expect("piped"), tx);
    let status = loop {
        if cancel.is_cancelled() {
            crate::process::kill_tree(&mut child);
            return Err(Error::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => on_line(line.trim_end()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break child.wait().map_err(|e| Error::io("waiting for uv", e))?
            }
        }
    };
    if status.success() {
        Ok(())
    } else {
        Err(Error::WhisperxSetup(format!(
            "`uv tool install whisperx` failed ({status})"
        )))
    }
}
// ------------------------------------------------------------------------------------------------- the run

/// Runs WhisperX on the recording and returns what it found.
///
/// `language` is already validated (`auto` or a code). The recording is first converted to a 16 kHz WAV in a
/// private temporary directory, which is removed whatever happens.
pub fn run(
    exe: &Path,
    audio: &Path,
    options: &Options,
    language: &str,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<Output> {
    let model = model_name(&options.model)?;
    let tmp = tempfile::Builder::new()
        .prefix("nastro-whisperx-")
        .tempdir()
        .map_err(|e| Error::io("creating a temporary folder", e))?;
    let wav = tmp.path().join("audio.wav");

    on_progress(Progress::Decoding);
    let audio_seconds = crate::audio::decode_to_wav(audio, &wav, cancel)?;
    cancel.check()?;

    let mut cmd = Command::new(exe);
    cmd.args(args(
        &wav,
        language,
        &model,
        tmp.path(),
        crate::effective_threads(options.threads),
        options.diarize,
        options.speakers,
    ))
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    // Python's own text encoding is the console's by default, which garbles (or crashes on) non-ASCII output.
    .env("PYTHONIOENCODING", "utf-8")
    .env("PYTHONUTF8", "1")
    .env("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
    .env("TOKENIZERS_PARALLELISM", "false");
    if options.diarize {
        if let Some(token) = &options.hf_token {
            cmd.env("HF_TOKEN", token.expose());
        }
    }
    crate::process::configure(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| Error::WhisperxSetup(format!("could not start {}: {e}", exe.display())))?;
    let (tx, rx) = mpsc::channel();
    pump(child.stdout.take().expect("piped"), tx.clone());
    pump(child.stderr.take().expect("piped"), tx);

    let mut tail: VecDeque<String> = VecDeque::new();
    let mut phase: Option<Phase> = None;
    let mut handle =
        |line: String, tail: &mut VecDeque<String>, on_progress: &mut dyn FnMut(Progress)| {
            if let Some(p) = phase_of(&line) {
                if phase != Some(p) {
                    phase = Some(p);
                    on_progress(Progress::Phase(p));
                }
            }
            if tail.len() >= 80 {
                tail.pop_front();
            }
            tail.push_back(line);
        };

    let status = loop {
        if cancel.is_cancelled() {
            crate::process::kill_tree(&mut child);
            return Err(Error::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => handle(line, &mut tail, on_progress),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break child
                    .wait()
                    .map_err(|e| Error::io("waiting for WhisperX", e))?;
            }
        }
        // The pipes can stay open after the program has gone (a stray descendant holding them), so also ask directly.
        if let Some(status) = child
            .try_wait()
            .map_err(|e| Error::io("waiting for WhisperX", e))?
        {
            let drain_until = std::time::Instant::now() + Duration::from_millis(300);
            while let Ok(line) =
                rx.recv_timeout(drain_until.saturating_duration_since(std::time::Instant::now()))
            {
                handle(line, &mut tail, on_progress);
            }
            break status;
        }
    };

    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if !status.success() {
        let tail: Vec<String> = tail.into_iter().collect();
        return Err(classify_failure(
            &tail,
            &status.to_string(),
            options.hf_token.as_ref(),
        ));
    }

    let json_path = tmp.path().join("audio.json");
    let json = std::fs::read(&json_path).map_err(|e| {
        Error::WhisperxOutput(format!("WhisperX finished but wrote no result ({e})"))
    })?;
    let (segments, detected) = parse_output(&json)?;
    Ok(Output {
        segments,
        language: detected.unwrap_or_else(|| language.to_string()),
        audio_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn lossy(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn tok(s: &str) -> Secret {
        Secret::new(s).unwrap()
    }

    // ----- model names

    #[test]
    fn model_names_map_to_whisperx_names() {
        assert_eq!(model_name("large-v3-turbo").unwrap(), "large-v3-turbo");
        assert_eq!(model_name("small.en").unwrap(), "small.en");
        assert_eq!(
            model_name("base-q5_1").unwrap(),
            "base",
            "ggml quantisation suffixes are dropped"
        );
        assert_eq!(model_name("large-v3-turbo-q8_0").unwrap(), "large-v3-turbo");
        assert!(matches!(
            model_name("nonsense"),
            Err(Error::WhisperxModel(_))
        ));
        assert!(matches!(
            model_name(r"C:\models\ggml-base.bin"),
            Err(Error::WhisperxModel(_))
        ));
        assert!(matches!(
            model_name("/opt/x.bin"),
            Err(Error::WhisperxModel(_))
        ));
    }

    // ----- argv

    #[test]
    fn argv_has_the_options_the_original_used_and_never_the_token() {
        let a = lossy(&args(
            Path::new("in.wav"),
            "en",
            "small",
            Path::new("out"),
            4,
            true,
            Speakers::default(),
        ));
        assert_eq!(a[0], "in.wav");
        let joined = a.join(" ");
        for want in [
            "--diarize",
            "--language en",
            "--model small",
            "--device cpu",
            "--compute_type int8",
            "--threads 4",
            "--output_dir out",
            "--output_format json",
        ] {
            assert!(joined.contains(want), "missing {want:?} in {joined}");
        }
        assert!(!joined.contains("hf_token") && !joined.contains("--token"));
    }

    #[test]
    fn auto_language_is_left_for_whisperx_to_detect() {
        for l in ["auto", ""] {
            let j = lossy(&args(
                Path::new("a"),
                l,
                "small",
                Path::new("o"),
                1,
                true,
                Speakers::default(),
            ))
            .join(" ");
            assert!(!j.contains("--language"), "{j}");
        }
    }

    #[test]
    fn speaker_counts_are_passed_only_when_diarizing() {
        let on = lossy(&args(
            Path::new("a"),
            "en",
            "m",
            Path::new("o"),
            1,
            true,
            Speakers::exactly(3),
        ))
        .join(" ");
        assert!(on.contains("--min_speakers 3") && on.contains("--max_speakers 3"));
        let off = lossy(&args(
            Path::new("a"),
            "en",
            "m",
            Path::new("o"),
            1,
            false,
            Speakers::exactly(3),
        ))
        .join(" ");
        assert!(
            !off.contains("--diarize") && !off.contains("speakers"),
            "{off}"
        );
        let range = lossy(&args(
            Path::new("a"),
            "en",
            "m",
            Path::new("o"),
            1,
            true,
            Speakers {
                min: Some(2),
                max: Some(5),
            },
        ))
        .join(" ");
        assert!(range.contains("--min_speakers 2") && range.contains("--max_speakers 5"));
    }

    #[test]
    fn paths_with_spaces_stay_single_arguments() {
        let a = args(
            Path::new("C:/My Dir/a b.wav"),
            "en",
            "small",
            Path::new("C:/Out Dir"),
            2,
            true,
            Speakers::default(),
        );
        assert!(a.iter().any(|x| x == OsStr::new("C:/My Dir/a b.wav")));
        assert!(a.iter().any(|x| x == OsStr::new("C:/Out Dir")));
    }

    // ----- output

    #[test]
    fn parses_diarized_output() {
        let json = br#"{"language":"en","segments":[
            {"start":0.149,"end":1.25,"text":" Good morning everyone.","speaker":"SPEAKER_00","words":[{"word":"Good","start":0.1}]},
            {"start":2.23,"end":5.39,"text":"Thanks.","speaker":"SPEAKER_01"},
            {"start":6.0,"end":7.0,"text":"No speaker here."}
        ],"word_segments":[]}"#;
        let (segs, lang) = parse_output(json).unwrap();
        assert_eq!(lang.as_deref(), Some("en"));
        assert_eq!(segs.len(), 3);
        assert_eq!((segs[0].start_ms, segs[0].end_ms), (149, 1250));
        assert_eq!(segs[0].text, "Good morning everyone.", "text is trimmed");
        assert_eq!(segs[0].speaker.as_deref(), Some("SPEAKER_00"));
        assert_eq!(
            segs[2].speaker, None,
            "an unattributed segment keeps no speaker"
        );
    }

    #[test]
    fn parses_output_from_a_real_whisperx_run() {
        // Shape taken from an actual run (no diarization): segments carry words and avg_logprob, no speaker.
        let json = br#"{"segments":[{"start":0.149,"end":1.249,"text":" Good morning everyone.","words":[{"word":"Good","start":0.149,"end":0.309,"score":0.998}],"avg_logprob":-0.21}],"word_segments":[],"language":"en"}"#;
        let (segs, lang) = parse_output(json).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].speaker, None);
        assert_eq!(lang.as_deref(), Some("en"));
    }

    #[test]
    fn missing_or_odd_fields_are_tolerated() {
        let (segs, _) = parse_output(
            br#"{"segments":[{"text":"x","speaker":""},{"start":-3,"end":1,"text":"y"}]}"#,
        )
        .unwrap();
        assert_eq!(segs[0].speaker, None, "an empty speaker is no speaker");
        assert_eq!(segs[1].start_ms, 0, "negative times clamp to zero");
        assert_eq!(
            parse_output(b"{}").unwrap().0.len(),
            0,
            "no segments is an empty result, not an error"
        );
        // A value of the wrong type is a malformed file, rejected as a whole rather than half-read.
        assert!(parse_output(br#"{"segments":[{"start":1,"end":"n/a","text":"x"}]}"#).is_err());
    }
    #[test]
    fn garbage_output_is_a_clear_error() {
        assert!(matches!(
            parse_output(b"not json"),
            Err(Error::WhisperxOutput(_))
        ));
        assert!(matches!(parse_output(b""), Err(Error::WhisperxOutput(_))));
    }

    // ----- phases (real log lines)

    #[test]
    fn real_log_lines_map_to_phases() {
        let cases = [
            ("2026-10-23 20:26:37 - whisperx.vads.pyannote - INFO - Performing voice activity detection using Pyannote...", Some(Phase::DetectingSpeech)),
            ("2026-10-23 20:26:38 - whisperx.transcribe - INFO - Performing transcription...", Some(Phase::Transcribing)),
            ("2026-10-23 20:26:41 - whisperx.transcribe - INFO - Performing alignment...", Some(Phase::Aligning)),
            ("2026-10-23 20:27:10 - whisperx.transcribe - INFO - Performing diarization...", Some(Phase::Diarizing)),
            ("Transcript: [0.149 --> 27.875]  Good morning everyone.", None),
            ("torchcodec is not installed correctly so built-in audio decoding will fail.", None),
        ];
        for (line, want) in cases {
            assert_eq!(phase_of(line), want, "{line}");
        }
    }

    // ----- secrets

    #[test]
    fn the_token_is_scrubbed_from_text() {
        let t = tok("hf_abcdefghijklmnopqrstuvwxyz0123");
        let s = scrub(
            "auth failed for hf_abcdefghijklmnopqrstuvwxyz0123 at hub",
            Some(&t),
        );
        assert!(!s.contains("abcdefghij") && s.contains("***"), "{s}");
    }

    #[test]
    fn anything_shaped_like_a_token_is_scrubbed_even_if_it_is_not_ours() {
        let s = scrub(
            "saw hf_ZZZZZZZZZZZZZZZZZZZZZZZZ and also hf_short and hf_",
            None,
        );
        assert!(!s.contains("ZZZZZ"));
        assert!(
            s.contains("hf_short"),
            "short look-alikes are left alone: {s}"
        );
        assert!(s.ends_with("hf_"));
    }

    #[test]
    fn debug_output_never_shows_the_token() {
        let t = tok("hf_super_secret_value_1234567890");
        assert_eq!(format!("{t:?}"), "Secret(***)");
        let o = Options {
            hf_token: Some(t),
            ..Options::default()
        };
        assert!(!format!("{o:?}").contains("super_secret"), "{o:?}");
    }

    // ----- failures

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn missing_nltk_data_is_recognised_from_the_real_error() {
        let log = lines(
            "LookupError: \n****\n  Resource 'punkt_tab' not found.\n  Please use the NLTK Downloader to obtain the resource:\n  >>> import nltk\n  >>> nltk.download('punkt_tab')",
        );
        let e = classify_failure(&log, "exit code: 1", None);
        assert!(matches!(e, Error::WhisperxSetup(_)), "{e}");
        assert!(e.to_string().contains("--setup-whisperx"));
    }

    #[test]
    fn a_gated_model_names_the_page_to_accept_terms_on() {
        let log = lines("huggingface_hub.errors.GatedRepoError: 403 Client Error.\nCannot access gated repo for url https://huggingface.co/pyannote/speaker-diarization-community-1/resolve/main/config.yaml.\nAccess to model pyannote/speaker-diarization-community-1 is restricted. You must have access to it and be authenticated to access it.");
        match classify_failure(&log, "exit code: 1", None) {
            Error::DiarizationAccess { repo } => {
                assert_eq!(repo, "pyannote/speaker-diarization-community-1")
            }
            e => panic!("wrong error: {e}"),
        }
        let other =
            lines("GatedRepoError: Access to model pyannote/segmentation-3.0 is restricted.");
        assert!(
            matches!(classify_failure(&other, "x", None), Error::DiarizationAccess { repo } if repo == "pyannote/segmentation-3.0")
        );
    }

    #[test]
    fn a_bad_token_is_told_apart_from_a_missing_terms_acceptance() {
        // Verbatim from a real run of WhisperX 3.8.6 with an invalid token (401), which is a *token* problem.
        let bad_token = lines(
            "huggingface_hub.errors.GatedRepoError: 401 Client Error. (Request ID: Root=1-6adc2a14-30d7aa2020db23e805cb565f)\nCannot access gated repo for url https://huggingface.co/pyannote/speaker-diarization-community-1/resolve/main/config.yaml.\nCould not download Pipeline from pyannote/speaker-diarization-community-1.\nIt might be because the repository is private or gated:\n* visit https://hf.co/pyannote/speaker-diarization-community-1 to accept user conditions\n* visit https://hf.co/settings/tokens to create an authentication token",
        );
        assert!(matches!(
            classify_failure(&bad_token, "x", None),
            Error::HfTokenRejected
        ));

        // A valid token that has not accepted the terms gets a 403 and the "restricted" wording.
        let no_terms = lines(
            "huggingface_hub.errors.GatedRepoError: 403 Client Error.\nAccess to model pyannote/speaker-diarization-community-1 is restricted and you are not in the authorized list.",
        );
        assert!(matches!(
            classify_failure(&no_terms, "x", None),
            Error::DiarizationAccess { .. }
        ));
    }

    #[test]
    fn a_rejected_token_is_reported_as_such() {
        let log = lines("requests.exceptions.HTTPError: 401 Client Error: Unauthorized for url: https://huggingface.co/api/whoami-v2\nInvalid user token.");
        assert!(matches!(
            classify_failure(&log, "x", None),
            Error::HfTokenRejected
        ));
    }

    #[test]
    fn other_failures_show_the_last_meaningful_lines_without_the_noise_or_the_token() {
        let t = tok("hf_abcdefghijklmnopqrstuvwxyz0123");
        let mut log = lines("torchcodec is not installed correctly so built-in audio decoding will fail.\nCould not load libtorchcodec.\n[end of libtorchcodec loading traceback].");
        log.push("RuntimeError: something broke using hf_abcdefghijklmnopqrstuvwxyz0123".into());
        log.push("  at the end".into());
        match classify_failure(&log, "exit code: 1", Some(&t)) {
            Error::WhisperxFailed { status, detail } => {
                assert_eq!(status, "exit code: 1");
                assert!(detail.contains("RuntimeError") && detail.contains("at the end"));
                assert!(!detail.contains("torchcodec"), "noise filtered: {detail}");
                assert!(!detail.contains("abcdefghij"), "token scrubbed: {detail}");
            }
            e => panic!("wrong error: {e}"),
        }
    }

    #[test]
    fn a_missing_python_module_is_a_setup_problem() {
        let log = lines("ModuleNotFoundError: No module named 'pyannote'");
        assert!(matches!(
            classify_failure(&log, "x", None),
            Error::WhisperxSetup(_)
        ));
    }

    // ----- locating

    #[test]
    fn programs_are_found_in_the_first_directory_that_has_them() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(b.path().join("tool"), b"x").unwrap();
        std::fs::write(a.path().join("other"), b"x").unwrap();
        let dirs = [a.path().to_path_buf(), b.path().to_path_buf()];
        assert_eq!(first_in("tool", &dirs), Some(b.path().join("tool")));
        assert_eq!(first_in("missing", &dirs), None);
        std::fs::write(a.path().join("tool"), b"x").unwrap();
        assert_eq!(
            first_in("tool", &dirs),
            Some(a.path().join("tool")),
            "earlier directories win"
        );
    }
}

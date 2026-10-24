//! `nastro transcribe`: speech-to-text for a saved recording.

use std::io::{BufRead, IsTerminal, Write};
use std::time::Duration;

use anyhow::{bail, Context};
use nastro_config::Config;
use nastro_transcribe::{
    models, whisperx, Backend, CancelToken, Error, Options, Progress, Speakers,
};

#[derive(clap::Args)]
pub struct Args {
    /// Recording id (its directory name), or "last" for the newest.
    #[arg(
        required_unless_present_any = ["list_models", "setup_whisperx"],
        value_name = "ID|last"
    )]
    pub id: Option<String>,

    /// Replace an existing transcript without asking.
    #[arg(long)]
    pub force: bool,

    /// Answer yes to prompts, including downloading a missing model.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Model name (see --list-models) or path to a ggml .bin file. Overrides `whisper_model` in the config.
    #[arg(long, value_name = "NAME|PATH")]
    pub model: Option<String>,

    /// Language code such as `en`, or `auto`. Overrides `lang` in the config.
    #[arg(long, value_name = "CODE")]
    pub lang: Option<String>,

    /// CPU threads to use (default: this machine's cores, up to 16).
    #[arg(long, value_name = "N")]
    pub threads: Option<usize>,

    /// Engine: `whisper` (built in, fast) or `whisperx` (also labels who is speaking; needs a one-time setup, see
    /// --setup-whisperx). Overrides `transcriber` in the config.
    #[arg(long, value_name = "whisper|whisperx")]
    pub transcriber: Option<String>,

    /// Number of speakers, if you know it (whisperx only; it improves the labels).
    #[arg(long, value_name = "N", conflicts_with_all = ["min_speakers", "max_speakers"])]
    pub speakers: Option<u32>,

    /// Fewest speakers to expect (whisperx only).
    #[arg(long, value_name = "N")]
    pub min_speakers: Option<u32>,

    /// Most speakers to expect (whisperx only).
    #[arg(long, value_name = "N")]
    pub max_speakers: Option<u32>,

    /// Install WhisperX (with `uv`) and fetch its one-off data, then exit.
    #[arg(long)]
    pub setup_whisperx: bool,

    /// List the models that can be downloaded, and exit.
    #[arg(long)]
    pub list_models: bool,
}

pub fn run(config: &Config, args: Args) -> anyhow::Result<()> {
    if args.list_models {
        print_models(
            &args
                .model
                .clone()
                .unwrap_or_else(|| config.whisper_model.clone()),
        );
        return Ok(());
    }
    if args.setup_whisperx {
        return setup_whisperx(config);
    }
    let id = args
        .id
        .as_deref()
        .expect("clap requires an id unless --list-models or --setup-whisperx");

    let backend = match &args.transcriber {
        Some(name) => Backend::from_name(name)?,
        None => nastro_transcribe::check_transcriber(&config.transcriber)?,
    };
    if args.threads == Some(0) {
        bail!("--threads must be at least 1");
    }
    let speakers = Speakers {
        min: args.speakers.or(args.min_speakers),
        max: args.speakers.or(args.max_speakers),
    };
    if [speakers.min, speakers.max].contains(&Some(0)) {
        bail!("the number of speakers must be at least 1");
    }
    if speakers.min.zip(speakers.max).is_some_and(|(a, b)| a > b) {
        bail!("--min-speakers cannot be larger than --max-speakers");
    }
    if backend != Backend::WhisperX && speakers != Speakers::default() {
        bail!("the speaker options only apply to the speaker-labelling engine: add `--transcriber whisperx`");
    }

    // Which recording?
    let records = nastro_records::scan(&config.output_dir)
        .with_context(|| format!("reading recordings in {}", config.output_dir.display()))?;
    let record = nastro_transcribe::resolve(&records, id)?;
    let newest = nastro_transcribe::resolve(&records, "last")
        .ok()
        .map(|r| r.dir_name.as_str());
    if nastro_transcribe::recording_in_progress(
        &config.output_dir,
        newest,
        &record.dir_name,
        &nastro_record::is_process_alive,
    ) {
        return Err(Error::InProgress.into());
    }

    let mut options = Options::from_config(config);
    options.backend = backend;
    options.speakers = speakers;
    if let Some(model) = &args.model {
        options.model = model.clone();
    }
    if let Some(lang) = &args.lang {
        options.lang = lang.clone();
    }
    options.threads = args.threads;
    if let Some(requested) = args.threads {
        let used = nastro_transcribe::effective_threads(Some(requested));
        if used < requested {
            eprintln!("note: using {used} threads, not {requested}: more than this computer's cores makes recognition dramatically slower, not faster.");
        }
    }

    // Existing transcript: ask, unless told.
    let existing = record.path.join(nastro_transcribe::output::TXT_FILE);
    if existing.exists() {
        // A script (no terminal, nobody to ask) must be told, not silently left with the old transcript.
        if !args.force && !args.yes && !std::io::stdin().is_terminal() {
            return Err(Error::AlreadyTranscribed(existing).into());
        }
        if args.force
            || confirm(
                &format!(
                    "{} is already transcribed. Overwrite it? [y/N] ",
                    record.dir_name
                ),
                args.yes,
            )
        {
            options.overwrite = true;
        } else {
            outln!("Nothing changed.");
            return Ok(());
        }
    }

    if backend == Backend::WhisperX {
        prepare_whisperx(&options, args.yes)?;
    } else if !models::is_path(&options.model) && !models::is_installed(&options.model) {
        // A missing model is a (possibly multi-gigabyte) download: never silently.
        let Some(info) = models::find(&options.model) else {
            return Err(Error::UnknownModel(options.model.clone()).into());
        };
        let prompt = format!(
            "The speech model '{}' ({}) is not on this computer yet.\nDownload it from huggingface.co to {} ? [y/N] ",
            info.name,
            nastro_records::format_size(info.size),
            models::model_dir().display()
        );
        if !confirm(&prompt, args.yes) {
            bail!(
                "model '{}' is not installed. Re-run with --yes to download it, or pick a smaller one with --model (see --list-models)",
                info.name
            );
        }
    }

    // Ctrl+C cancels cleanly (any previous transcript is left as it was).
    let cancel = CancelToken::new();
    {
        let c = cancel.clone();
        ctrlc::set_handler(move || c.cancel()).context("installing the Ctrl+C handler")?;
    }

    let mut reporter = Reporter::new(std::io::stderr().is_terminal(), &options.model);
    let result =
        nastro_transcribe::transcribe(&record.path, &options, &cancel, &mut |p| reporter.update(p));
    reporter.finish();

    match result {
        Ok(out) => {
            if out.segments == 0 {
                outln!("No speech was detected; wrote an empty transcript.");
            } else {
                let who = match out.speakers {
                    0 => String::new(),
                    1 => ", 1 speaker".to_string(),
                    n => format!(", {n} speakers"),
                };
                outln!(
                    "Transcribed {} of audio in {} (language: {}, {} segments{who}).",
                    nastro_records::format_duration(out.audio_seconds),
                    format_elapsed(out.elapsed),
                    out.language,
                    out.segments
                );
            }
            outln!(
                "  {}",
                record
                    .path
                    .join(nastro_transcribe::output::TXT_FILE)
                    .display()
            );
            outln!(
                "  {}",
                record
                    .path
                    .join(nastro_transcribe::output::SRT_FILE)
                    .display()
            );
            Ok(())
        }
        Err(Error::Cancelled) => {
            eprintln!("Cancelled; nothing was changed.");
            std::process::exit(130);
        }
        Err(e) => Err(e.into()),
    }
}

/// Everything WhisperX needs that can be settled before the long run: a valid model name, a Hugging Face token, and
/// the program itself (offered for install, with consent, when it is missing).
fn prepare_whisperx(options: &Options, assume_yes: bool) -> anyhow::Result<()> {
    whisperx::model_name(&options.model)?;
    // The token first: there is no point offering a two-gigabyte install to someone who cannot then use it.
    if options.diarize && options.hf_token.is_none() {
        return Err(Error::HfTokenMissing.into());
    }
    if whisperx::locate().is_none() {
        let missing = Error::WhisperxMissing {
            searched: "PATH and ~/.local/bin".into(),
        };
        if whisperx::locate_uv().is_none() {
            return Err(missing.into());
        }
        let prompt = "WhisperX (speaker labels) is not installed. Install it now with uv?\nIt is about 2 GB, and installs only for your user. [y/N] ";
        if !confirm(prompt, assume_yes) {
            return Err(missing.into());
        }
        run_install()?;
    }
    eprintln!(
        "note: speaker identification downloads its speech and speaker models the first time (several GB) and runs\non the CPU, so it is much slower than plain transcription."
    );
    Ok(())
}

/// Installs WhisperX, streaming the installer's output, with Ctrl+C to cancel.
fn run_install() -> anyhow::Result<()> {
    let cancel = CancelToken::new();
    {
        let c = cancel.clone();
        // A handler may already exist in this process; the first one is enough.
        let _ = ctrlc::set_handler(move || c.cancel());
    }
    match whisperx::install(&mut |line| eprintln!("{line}"), &cancel) {
        Ok(()) => Ok(()),
        Err(Error::Cancelled) => {
            eprintln!("Cancelled.");
            std::process::exit(130);
        }
        Err(e) => Err(e.into()),
    }
}

/// `nastro transcribe --setup-whisperx`.
fn setup_whisperx(config: &Config) -> anyhow::Result<()> {
    run_install()?;
    outln!("WhisperX is installed.");
    if Options::from_config(config).hf_token.is_none() {
        outln!();
        outln!("One more step before it can label speakers:");
        outln!("  1. Create a free *read* token at https://huggingface.co/settings/tokens");
        outln!("  2. Accept the model terms at https://huggingface.co/pyannote/speaker-diarization-community-1");
        outln!(
            "  3. Put the token in the config file as `hf_token = \"hf_...\"` (or set HF_TOKEN)"
        );
    }
    outln!();
    outln!("Then: nastro transcribe last --transcriber whisperx   (or set `transcriber = \"whisperx\"`)");
    Ok(())
}

fn print_models(selected: &str) {
    outln!("{:<22} {:>10}  INSTALLED", "MODEL", "SIZE");
    for m in nastro_transcribe::MODELS {
        let installed = if models::is_installed(m.name) {
            "yes"
        } else {
            ""
        };
        let mark = if m.name == selected {
            "  <- selected"
        } else {
            ""
        };
        outln!(
            "{:<22} {:>10}  {}{}",
            m.name,
            nastro_records::format_size(m.size),
            installed,
            mark
        );
    }
    outln!();
    outln!("Models are stored in {}", models::model_dir().display());
    outln!("Set the default with `whisper_model = \"...\"` in the config file. `.en` models are English-only and faster.");
}

/// Asks a yes/no question. Never blocks a script: without a terminal the answer is "no" unless `assume_yes`.
fn confirm(prompt: &str, assume_yes: bool) -> bool {
    if assume_yes {
        return true;
    }
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).is_ok() && is_yes(&line)
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 {
        format!("{s:.1}s")
    } else {
        let t = d.as_secs();
        format!("{}m{:02}s", t / 60, t % 60)
    }
}

/// One status line for a progress event.
fn render(p: Progress, model: &str) -> String {
    match p {
        Progress::DownloadingModel { done, total } => {
            let pct = (done * 100).checked_div(total).unwrap_or(0);
            format!(
                "Downloading model {model}: {pct}%  {} / {}",
                nastro_records::format_size(done),
                nastro_records::format_size(total)
            )
        }
        Progress::Decoding => "Reading audio...".to_string(),
        Progress::Phase(p) => {
            let mut label = p.label().to_string();
            if let Some(first) = label.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            format!("{label}...")
        }
        Progress::Transcribing { percent } => format!("Transcribing: {percent}%"),
        Progress::Saving => "Saving...".to_string(),
    }
}

/// Collapses a progress event to (stage, decile), so a non-terminal log gets one line per stage and one per
/// 10% of the long stages rather than one per tick.
fn plain_key(p: Progress) -> (u8, u64) {
    match p {
        Progress::DownloadingModel { done, total } => {
            (0, (done * 10).checked_div(total).unwrap_or(0))
        }
        Progress::Decoding => (1, 0),
        Progress::Phase(p) => (4, p as u64),
        Progress::Transcribing { percent } => (2, u64::from(percent) / 10),
        Progress::Saving => (3, 0),
    }
}

/// Shows progress on stderr: a single rewritten line on a terminal, and a few plain lines otherwise (so logs
/// and pipes are not flooded).
struct Reporter {
    tty: bool,
    model: String,
    shown_len: usize,
    last_key: Option<(u8, u64)>,
}

impl Reporter {
    fn new(tty: bool, model: &str) -> Self {
        Reporter {
            tty,
            model: model.to_string(),
            shown_len: 0,
            last_key: None,
        }
    }

    fn update(&mut self, p: Progress) {
        let line = render(p, &self.model);
        if self.tty {
            let pad = self.shown_len.saturating_sub(line.len());
            eprint!("\r{line}{}", " ".repeat(pad));
            let _ = std::io::stderr().flush();
            self.shown_len = line.len();
        } else if self.last_key != Some(plain_key(p)) {
            eprintln!("{line}");
        }
        self.last_key = Some(plain_key(p));
    }

    fn finish(&mut self) {
        if self.tty && self.shown_len > 0 {
            eprintln!();
            self.shown_len = 0;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yes_answers() {
        for a in ["y", "Y", "yes", " YES \n"] {
            assert!(is_yes(a), "{a:?}");
        }
        for a in ["", "n", "no", "yep", "ye", "\n"] {
            assert!(!is_yes(a), "{a:?}");
        }
    }

    #[test]
    fn assume_yes_never_reads_stdin() {
        assert!(confirm("ignored", true));
    }

    #[test]
    fn status_lines() {
        assert_eq!(render(Progress::Decoding, "m"), "Reading audio...");
        assert_eq!(
            render(Progress::Transcribing { percent: 37 }, "m"),
            "Transcribing: 37%"
        );
        assert_eq!(render(Progress::Saving, "m"), "Saving...");
        let d = render(
            Progress::DownloadingModel {
                done: 50,
                total: 200,
            },
            "tiny.en",
        );
        assert!(d.starts_with("Downloading model tiny.en: 25%"), "{d}");
        // an unknown total must not divide by zero
        assert!(render(Progress::DownloadingModel { done: 5, total: 0 }, "m").contains("0%"));
    }

    #[test]
    fn a_log_gets_one_line_per_stage_and_decile_not_per_tick() {
        let t = |percent| plain_key(Progress::Transcribing { percent });
        assert_eq!(t(41), t(49));
        assert_ne!(t(49), t(50));
        assert_ne!(plain_key(Progress::Decoding), t(0));
        assert_eq!(
            plain_key(Progress::DownloadingModel {
                done: 1,
                total: 100
            }),
            plain_key(Progress::DownloadingModel {
                done: 9,
                total: 100
            })
        );
        assert_eq!(
            plain_key(Progress::DownloadingModel { done: 5, total: 0 }).1,
            0
        );
    }

    #[test]
    fn elapsed_formatting() {
        assert_eq!(format_elapsed(Duration::from_millis(10_900)), "10.9s");
        assert_eq!(format_elapsed(Duration::from_secs(125)), "2m05s");
    }
}

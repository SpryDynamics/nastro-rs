//! `nastro transcribe`: speech-to-text for a saved recording.

use std::io::{BufRead, IsTerminal, Write};
use std::time::Duration;

use anyhow::{bail, Context};
use nastro_config::Config;
use nastro_transcribe::{models, CancelToken, Error, Options, Progress};

#[derive(clap::Args)]
pub struct Args {
    /// Recording id (its directory name), or "last" for the newest.
    #[arg(required_unless_present = "list_models", value_name = "ID|last")]
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
    let id = args
        .id
        .as_deref()
        .expect("clap requires an id unless --list-models");

    nastro_transcribe::check_transcriber(&config.transcriber)?;
    if args.threads == Some(0) {
        bail!("--threads must be at least 1");
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

    // A missing model is a (possibly multi-gigabyte) download: never silently.
    if !models::is_path(&options.model) && !models::is_installed(&options.model) {
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
                outln!(
                    "Transcribed {} of audio in {} (language: {}, {} segments).",
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

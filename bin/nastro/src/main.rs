#[macro_use]
mod out;
mod transcribe_cmd;

use clap::{Parser, Subcommand};
use nastro_config::Config;
use nastro_record::{CaptureMode, Outcome, RecordingSession, SessionError};

#[derive(Parser)]
#[command(
    name = "nastro",
    version,
    about = "Terminal-native call recorder: system audio + mic, local files, on-device transcription."
)]
struct Cli {
    /// Use this config file instead of the default location (also: NASTRO_CONFIG).
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Headless recording: Ctrl+C stops & saves.
    Record {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, conflicts_with = "system_only")]
        mic_only: bool,
        #[arg(long, conflicts_with = "mic_only")]
        system_only: bool,
    },
    /// List recordings.
    Records {
        /// Scriptable, tab-separated output.
        #[arg(long)]
        plain: bool,
    },
    /// Transcribe a recording (speech to text, on this computer).
    Transcribe(transcribe_cmd::Args),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let explicit = cli
        .config
        .or_else(|| std::env::var_os("NASTRO_CONFIG").map(std::path::PathBuf::from));
    let config = load_config(explicit.as_deref())?;

    match cli.command {
        None => run_tui_or_list(&config),
        Some(Command::Record {
            name,
            mic_only,
            system_only,
        }) => {
            let mode = if mic_only {
                CaptureMode::MicOnly
            } else if system_only {
                CaptureMode::SystemOnly
            } else {
                CaptureMode::Mixed
            };
            run_record(&config, name, mode)
        }
        Some(Command::Records { plain: true }) => run_records(&config, true),
        Some(Command::Records { plain: false }) => run_tui_or_list(&config),
        Some(Command::Transcribe(args)) => transcribe_cmd::run(&config, args),
    }
}

fn run_record(config: &Config, name: Option<String>, mode: CaptureMode) -> anyhow::Result<()> {
    let session = match RecordingSession::start(&config.output_dir, name.as_deref(), mode) {
        Ok(session) => session,
        Err(err @ SessionError::AlreadyRecording { .. }) | Err(err @ SessionError::TapNotFound) => {
            eprintln!("error: {err}.");
            std::process::exit(1);
        }
        Err(err) => return Err(err.into()),
    };

    if let Some(warning) = session.warning() {
        eprintln!("{warning}");
    }
    println!("Recording to {}", session.record_dir().display());
    println!("Press Ctrl+C to stop...");

    let stop_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handler_flag = std::sync::Arc::clone(&stop_requested);
    ctrlc::set_handler(move || {
        handler_flag.store(true, std::sync::atomic::Ordering::SeqCst);
    })?;

    // An Option so the session can be consumed by `stop()` on one path and still be
    // dropped (releasing the lock) before any `process::exit`, which skips destructors.
    let mut session = Some(session);
    let end = loop {
        let live = session
            .as_mut()
            .expect("session is only taken on the way out");
        if let Some(end) = live.try_finish() {
            break end;
        }
        if stop_requested.load(std::sync::atomic::Ordering::SeqCst) {
            break session.take().expect("still live").stop();
        }
        let secs = live.elapsed().as_secs();
        print!("\r{:02}:{:02}  ", secs / 60, secs % 60);
        let _ = std::io::Write::flush(&mut std::io::stdout());
        std::thread::sleep(std::time::Duration::from_millis(250));
    };
    println!();
    drop(session); // release the lock and sleep guard before we may exit()

    match end.outcome {
        Outcome::Saved => {
            println!("Saved {}", end.record_dir.display());
            Ok(())
        }
        Outcome::DiscardedTooShort => {
            println!("Recording was under 2 seconds - discarded.");
            Ok(())
        }
        Outcome::DiscardedByUser => {
            println!("Recording discarded.");
            Ok(())
        }
        Outcome::FailedNoAudio => {
            eprintln!("{}", end.exit.message);
            std::process::exit(1);
        }
    }
}
fn load_config(explicit: Option<&std::path::Path>) -> anyhow::Result<Config> {
    let home = directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    match explicit {
        // A path the user asked for must load: silently falling back to defaults
        // would record into a directory they did not choose.
        Some(path) => {
            // `load_from` treats a missing file as "use defaults", which is right for
            // the implicit location but wrong here: a typo in --config must not
            // quietly record into a directory the user never chose.
            anyhow::ensure!(
                path.is_file(),
                "config file not found: {} (set by --config or NASTRO_CONFIG)",
                path.display()
            );
            Ok(Config::load_from(path, &home)?)
        }
        None => Ok(Config::load().unwrap_or_else(|err| {
            eprintln!("warning: failed to load config ({err}); using defaults");
            Config::default_with_home(&home)
        })),
    }
}

/// Open the TUI when attached to a terminal; otherwise (piped, redirected, CI) fall
/// back to the plain table so scripts never get an unusable full-screen UI.
fn run_tui_or_list(config: &Config) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        Ok(nastro_tui::run(config)?)
    } else {
        run_records(config, false)
    }
}

fn run_records(config: &Config, plain: bool) -> anyhow::Result<()> {
    let records = nastro_records::scan(&config.output_dir)?;

    if plain {
        for record in &records {
            outln!("{}", nastro_records::plain_line(record));
        }
        return Ok(());
    }

    if records.is_empty() {
        outln!("No recordings yet in {}.", config.output_dir.display());
        return Ok(());
    }

    for record in &records {
        let duration = record
            .duration_seconds
            .map(nastro_records::format_duration)
            .unwrap_or_else(|| "-".to_string());
        let transcript = if record.has_transcript { "✓" } else { " " };
        outln!(
            "{:<32} {:>10} {:>10}  {}",
            record.dir_name,
            duration,
            nastro_records::format_size(record.size_bytes),
            transcript
        );
    }

    Ok(())
}

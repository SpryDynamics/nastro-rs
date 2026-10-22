use chrono::{Datelike, Timelike};
use clap::{Parser, Subcommand};
use nastro_config::Config;
use nastro_record::{CaptureMode, LockError, TapExitKind};

#[derive(Parser)]
#[command(
    name = "nastro",
    version,
    about = "Terminal-native call recorder: system audio + mic, local files, on-device transcription."
)]
struct Cli {
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
    /// Transcribe a recording by id, or "last".
    Transcribe { id: String },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let config = Config::load().unwrap_or_else(|err| {
        eprintln!("warning: failed to load config ({err}); using defaults");
        let home = directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        Config::default_with_home(&home)
    });

    match cli.command {
        None => {
            println!("nastro: TUI not yet implemented in the Rust port (coming in a later phase).");
            println!("Use `nastro records --plain` or `nastro record` in the meantime.");
            Ok(())
        }
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
        Some(Command::Records { plain }) => run_records(&config, plain),
        Some(Command::Transcribe { id }) => {
            println!("nastro transcribe {id}: not yet implemented in the Rust port.");
            Ok(())
        }
    }
}

fn run_record(config: &Config, name: Option<String>, mode: CaptureMode) -> anyhow::Result<()> {
    if let Ok(Some(warning)) = nastro_record::check_low_disk_space(&config.output_dir) {
        eprintln!("{warning}");
    }

    let lock =
        match nastro_record::acquire_lock(&config.output_dir, &nastro_record::is_process_alive) {
            Ok(lock) => lock,
            Err(LockError::AlreadyRecording { pid }) => {
                eprintln!("error: a recording is already in progress (pid {pid}).");
                std::process::exit(1);
            }
            Err(err) => return Err(err.into()),
        };

    let now = chrono::Local::now();
    let slug = name
        .as_deref()
        .map(nastro_record::slugify)
        .filter(|s| !s.is_empty());
    let dir_name = nastro_records::build_dir_name(
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        slug.as_deref(),
    );
    let record_dir = config.output_dir.join(&dir_name);
    std::fs::create_dir_all(&record_dir)?;
    let _ = nastro_config::mode::ensure_private_dir(&record_dir);
    let audio_path = record_dir.join("audio.m4a");

    let Some(tap_path) = nastro_record::resolve_tap_path() else {
        eprintln!("error: nastro-tap not found next to the current executable or on PATH.");
        drop(lock);
        std::process::exit(1);
    };

    println!("Recording to {}", record_dir.display());

    let mut tap = nastro_record::TapProcess::spawn(&tap_path, &audio_path, mode)?;
    let pid = tap.pid();

    let stop_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_requested_handler = std::sync::Arc::clone(&stop_requested);
    ctrlc::set_handler(move || {
        stop_requested_handler.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = nastro_record::request_graceful_stop(pid);
    })?;

    println!("Press Ctrl+C to stop...");

    let desc = loop {
        if let Some(desc) = tap.try_exit() {
            break desc;
        }
        if stop_requested.load(std::sync::atomic::Ordering::SeqCst) {
            break tap.wait_with_timeout(std::time::Duration::from_secs(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    drop(lock);

    match desc.kind {
        TapExitKind::Clean => {
            println!("{}", desc.message);
            Ok(())
        }
        TapExitKind::PermissionDenied | TapExitKind::Failed => {
            eprintln!("{}", desc.message);
            std::process::exit(1);
        }
    }
}

fn run_records(config: &Config, plain: bool) -> anyhow::Result<()> {
    let records = nastro_records::scan(&config.output_dir)?;

    if plain {
        for record in &records {
            println!("{}", nastro_records::plain_line(record));
        }
        return Ok(());
    }

    if records.is_empty() {
        println!("No recordings yet in {}.", config.output_dir.display());
        return Ok(());
    }

    for record in &records {
        let duration = record
            .duration_seconds
            .map(nastro_records::format_duration)
            .unwrap_or_else(|| "-".to_string());
        let transcript = if record.has_transcript { "✓" } else { " " };
        println!(
            "{:<32} {:>10} {:>10}  {}",
            record.dir_name,
            duration,
            nastro_records::format_size(record.size_bytes),
            transcript
        );
    }

    Ok(())
}

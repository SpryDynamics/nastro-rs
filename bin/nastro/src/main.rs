use clap::{Parser, Subcommand};
use nastro_config::Config;

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
                "mic-only"
            } else if system_only {
                "system-only"
            } else {
                "mixed"
            };
            println!(
                "nastro record: capture not yet implemented in the Rust port (name={:?}, mode={mode}).",
                name
            );
            println!(
                "Output would be written under: {}",
                config.output_dir.display()
            );
            Ok(())
        }
        Some(Command::Records { plain }) => run_records(&config, plain),
        Some(Command::Transcribe { id }) => {
            println!("nastro transcribe {id}: not yet implemented in the Rust port.");
            Ok(())
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

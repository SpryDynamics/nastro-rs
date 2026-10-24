//! Terminal UI for nastro, built on `ratatui`.
//!
//! Layout: [`app`] is the pure state machine (keys in, [`app::Effect`]s out),
//! [`view`] renders it, [`actions`] holds the OS-level actions (trash, reveal), and
//! this module owns the terminal and the event loop that ties them together.

pub mod actions;
pub mod app;
pub mod jobs;
pub mod view;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nastro_config::Config;
use nastro_record::RecordingSession;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;

use app::{App, Effect};
use jobs::{JobMsg, Jobs};

/// How often the loop wakes to refresh the recording timer / meters even with no
/// input.
const TICK: Duration = Duration::from_millis(100);

fn scan(config: &Config) -> io::Result<Vec<nastro_records::Record>> {
    nastro_records::scan(&config.output_dir).map_err(io::Error::other)
}

/// Run the TUI until the user quits. Restores the terminal on every exit path,
/// including panics (ratatui installs a panic hook for this).
pub fn run(config: &Config) -> io::Result<()> {
    let records = scan(config)?;

    // In raw mode Ctrl+C is an ordinary key, so this only fires for the ways a
    // recording usually gets cut short from outside: closing the terminal window
    // (SIGHUP / CTRL_CLOSE), `kill` (SIGTERM), logoff or shutdown. Without it the
    // process would die at once, leaving the capture helper to notice on its own and
    // the recording unfinalized.
    let terminate = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&terminate);
    // Fails only if a handler is already installed; the flag then just never fires.
    let _ = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst));

    let mut terminal = ratatui::try_init()?;
    let result = event_loop(
        &mut terminal,
        config,
        App::new(records, config.output_dir.clone()),
        &terminate,
    );
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    config: &Config,
    mut app: App,
    terminate: &AtomicBool,
) -> io::Result<()> {
    let mut session: Option<RecordingSession> = None;
    // Dropped (cancelling and joining any transcription) on every way out of this function.
    let mut jobs = Jobs::new(config.resolved_max_parallel() as usize);

    loop {
        terminal.draw(|frame| view::draw(frame, &app))?;

        if terminate.load(Ordering::SeqCst) {
            // Save what has been recorded (the 2-second rule still applies) rather
            // than abandoning a live capture.
            if let Some(s) = session.take() {
                s.stop();
            }
            return Ok(());
        }

        if event::poll(TICK)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Release {
                    for effect in app.handle_key(key) {
                        if perform(effect, terminal, config, &mut app, &mut session, &mut jobs)? {
                            return Ok(());
                        }
                    }
                }
            }
        }

        // Background transcriptions: progress, completion, and queued jobs taking over freed slots.
        for msg in jobs.poll() {
            handle_job_msg(msg, config, &mut app)?;
        }

        // Tick: feed the live session into the UI, and notice if the capture
        // process ended on its own (for example, microphone permission denied).
        if let Some(s) = session.as_mut() {
            app.on_recording_tick(s.elapsed(), s.latest_levels());
            if let Some(end) = s.try_finish() {
                session = None;
                app.on_recording_ended(end.outcome, &end.dir_name, &end.exit.message);
                app.on_records(scan(config)?);
            }
        }
    }
}

/// Carry out one effect. Returns `Ok(true)` when the app should exit.
fn perform(
    effect: Effect,
    terminal: &mut DefaultTerminal,
    config: &Config,
    app: &mut App,
    session: &mut Option<RecordingSession>,
    jobs: &mut Jobs,
) -> io::Result<bool> {
    match effect {
        Effect::Quit => {
            // Not reachable while recording (the keys there stop the session), but
            // never abandon a live capture if that ever changes.
            if let Some(s) = session.take() {
                s.stop();
            }
            return Ok(true);
        }
        Effect::StartRecording { name, mode } => {
            match RecordingSession::start(&config.output_dir, name.as_deref(), mode) {
                Ok(s) => {
                    app.on_recording_started(s.dir_name(), s.mode(), s.warning().map(String::from));
                    *session = Some(s);
                }
                Err(e) => app.on_start_failed(&e.to_string()),
            }
        }
        Effect::StopRecording { discard } => {
            if let Some(s) = session.take() {
                // Stopping can take a moment (ffmpeg flushes its last fragment), so
                // paint a message first rather than appearing to hang.
                app.show_busy("Finalizing recording...");
                terminal.draw(|frame| view::draw(frame, app))?;
                let end = if discard { s.discard() } else { s.stop() };
                app.on_recording_ended(end.outcome, &end.dir_name, &end.exit.message);
                app.on_records(scan(config)?);
            }
        }
        Effect::DeleteRecord { path, dir_name } => match actions::move_to_trash(&path) {
            Ok(()) => {
                app.on_deleted(&dir_name);
                app.on_records(scan(config)?);
            }
            Err(msg) => app.on_action_error(&msg),
        },
        Effect::RevealRecord { path } => {
            if let Err(msg) = actions::reveal(&path) {
                app.on_action_error(&msg);
            }
        }
        Effect::LoadPreview { path } => app.on_preview(actions::read_transcript(&path)),
        Effect::StartTranscription {
            path,
            dir_name,
            overwrite,
            download_ok,
        } => start_transcription(config, app, jobs, path, dir_name, overwrite, download_ok),
        Effect::CancelTranscription { dir_name } => {
            // A queued job disappears at once; a running one reports its cancellation through `jobs.poll()`.
            if jobs.cancel(&dir_name) {
                app.on_transcribe_cancelled(&dir_name);
            }
        }
    }
    Ok(false)
}

/// Checks everything that can be checked up front, so mistakes show immediately instead of after a queue wait or a
/// download, then hands the job to the background runner.
fn start_transcription(
    config: &Config,
    app: &mut App,
    jobs: &mut Jobs,
    path: std::path::PathBuf,
    dir_name: String,
    overwrite: bool,
    download_ok: bool,
) {
    use nastro_transcribe::{engine, models};

    let backend = match nastro_transcribe::check_transcriber(&config.transcriber) {
        Ok(b) => b,
        Err(e) => return app.on_action_error(&one_line(&e)),
    };

    let newest = app.records.iter().map(|r| r.dir_name.as_str()).max();
    if nastro_transcribe::recording_in_progress(
        &config.output_dir,
        newest,
        &dir_name,
        &nastro_record::is_process_alive,
    ) {
        return app.on_action_error(&nastro_transcribe::Error::InProgress.to_string());
    }

    let mut options = nastro_transcribe::Options::from_config(config);
    options.overwrite = overwrite;
    if let Err(e) = engine::resolve_language(&options.model, &options.lang) {
        return app.on_action_error(&one_line(&e));
    }

    // Speaker identification (WhisperX) fetches its own models and has its own prerequisites; nothing here is
    // downloaded without the user, so just report what is missing. (`nastro transcribe --setup-whisperx` fixes it.)
    if backend == nastro_transcribe::Backend::WhisperX {
        if let Err(e) = nastro_transcribe::whisperx::model_name(&options.model) {
            return app.on_action_error(&one_line(&e));
        }
        if options.hf_token.is_none() {
            return app.on_action_error(&one_line(&nastro_transcribe::Error::HfTokenMissing));
        }
        if nastro_transcribe::whisperx::locate().is_none() {
            return app.on_action_error(&one_line(&nastro_transcribe::Error::WhisperxMissing {
                searched: "PATH and ~/.local/bin".into(),
            }));
        }
        let state = jobs.submit(dir_name.clone(), path, options);
        return app.on_transcribe_state(&dir_name, state);
    }

    if !models::is_path(&options.model) && !models::is_installed(&options.model) {
        let Some(info) = models::find(&options.model) else {
            return app.on_action_error(
                &nastro_transcribe::Error::UnknownModel(options.model).to_string(),
            );
        };
        if !download_ok {
            return app.on_model_download_needed(
                &dir_name,
                info.name,
                &nastro_records::format_size(info.size),
                overwrite,
            );
        }
    }

    let state = jobs.submit(dir_name.clone(), path, options);
    app.on_transcribe_state(&dir_name, state);
}

/// An error message on a single line, for the status bar: multi-line guidance is joined, and the longest
/// messages (a failing tool's output) are cut. The full text is what `nastro transcribe` prints.
fn one_line(e: &nastro_transcribe::Error) -> String {
    const MAX: usize = 220;
    let flat = e
        .to_string()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= MAX {
        flat
    } else {
        let cut: String = flat.chars().take(MAX).collect();
        format!("{cut}... (run `nastro transcribe` for the full message)")
    }
}

/// Applies one report from a background job to the UI.
fn handle_job_msg(msg: JobMsg, config: &Config, app: &mut App) -> io::Result<()> {
    match msg {
        JobMsg::State { dir_name, state } => {
            // A late progress tick must not resurrect a job that has already finished or been cancelled.
            if app.jobs.contains_key(&dir_name) {
                app.on_transcribe_state(&dir_name, state);
            }
        }
        JobMsg::Done { dir_name, result } => match result {
            Ok(out) => {
                let summary = if out.segments == 0 {
                    "no speech detected".to_string()
                } else {
                    format!(
                        "{} of audio in {}s",
                        nastro_records::format_duration(out.audio_seconds),
                        out.elapsed.as_secs().max(1)
                    )
                };
                app.on_transcribe_finished(&dir_name, &summary);
                app.on_records(scan(config)?);
                // If that recording is open, show its new transcript.
                if app.screen == app::Screen::Detail {
                    if let Some(r) = app.selected_record().filter(|r| r.dir_name == dir_name) {
                        let path = r.path.clone();
                        app.on_preview(actions::read_transcript(&path));
                    }
                }
            }
            Err(nastro_transcribe::Error::Cancelled) => app.on_transcribe_cancelled(&dir_name),
            Err(e) => app.on_transcribe_failed(&dir_name, &one_line(&e)),
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nastro_transcribe::Error;

    #[test]
    fn multi_line_guidance_becomes_one_status_line() {
        let s = one_line(&Error::HfTokenMissing);
        assert!(!s.contains('\n'), "{s}");
        assert!(
            s.contains("Hugging Face") && s.contains("huggingface.co/settings/tokens"),
            "{s}"
        );
    }

    #[test]
    fn very_long_messages_are_cut_with_a_pointer_to_the_full_text() {
        let e = Error::WhisperxFailed {
            status: "exit code: 1".into(),
            detail: "x".repeat(2000),
        };
        let s = one_line(&e);
        assert!(s.chars().count() < 320, "{}", s.chars().count());
        assert!(s.contains("nastro transcribe"), "{s}");
    }

    #[test]
    fn short_messages_are_left_alone() {
        assert_eq!(one_line(&Error::Cancelled), "cancelled");
    }
}

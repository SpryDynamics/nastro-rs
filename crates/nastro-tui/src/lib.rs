//! Terminal UI for nastro, built on `ratatui`.
//!
//! Layout: [`app`] is the pure state machine (keys in, [`app::Effect`]s out),
//! [`view`] renders it, [`actions`] holds the OS-level actions (trash, reveal), and
//! this module owns the terminal and the event loop that ties them together.

pub mod actions;
pub mod app;
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
                        if perform(effect, terminal, config, &mut app, &mut session)? {
                            return Ok(());
                        }
                    }
                }
            }
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
    }
    Ok(false)
}

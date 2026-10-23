//! TUI state machine. Pure: key events go in, [`Effect`]s come out, and nothing here
//! touches the filesystem, the terminal or a process - the event loop in `lib.rs`
//! performs the effects and feeds results back through the `on_*` methods. That
//! keeps every screen transition unit-testable.
//!
//! Behaviour follows the original project's `docs/flows.md`: list / name form /
//! recording / detail / help screens, `y`-to-confirm destructive actions, and
//! stop-and-save on `s` / `q` / Ctrl+C while recording.

use std::path::PathBuf;
use std::time::Duration;

use nastro_record::{CaptureMode, LevelReading, Outcome};
use nastro_records::Record;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    List,
    NameForm,
    Recording,
    Detail,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    ConfirmDelete { dir_name: String },
    ConfirmDiscard,
}

/// Side effects the event loop must perform on the app's behalf.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    StartRecording {
        name: Option<String>,
        mode: CaptureMode,
    },
    StopRecording {
        discard: bool,
    },
    DeleteRecord {
        path: PathBuf,
        dir_name: String,
    },
    RevealRecord {
        path: PathBuf,
    },
    LoadPreview {
        path: PathBuf,
    },
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NameForm {
    pub name: String,
    pub mode: CaptureMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordingView {
    pub dir_name: String,
    pub mode: CaptureMode,
    pub elapsed: Duration,
    pub sys_level: f32,
    pub mic_level: f32,
    pub warning: Option<String>,
}

pub struct App {
    pub screen: Screen,
    /// Where `?` returns to when help is dismissed.
    help_return: Screen,
    pub records: Vec<Record>,
    pub filter: String,
    pub filtering: bool,
    pub selected: usize,
    pub form: NameForm,
    pub recording: Option<RecordingView>,
    pub modal: Option<Modal>,
    pub status: Option<Status>,
    pub preview: Option<String>,
    pub output_dir: PathBuf,
}

/// Cycle order for the capture-mode picker.
pub fn next_mode(mode: CaptureMode) -> CaptureMode {
    match mode {
        CaptureMode::Mixed => CaptureMode::MicOnly,
        CaptureMode::MicOnly => CaptureMode::SystemOnly,
        CaptureMode::SystemOnly => CaptureMode::Mixed,
    }
}

pub fn mode_label(mode: CaptureMode) -> &'static str {
    match mode {
        CaptureMode::Mixed => "System + mic",
        CaptureMode::MicOnly => "Mic only",
        CaptureMode::SystemOnly => "System only",
    }
}

impl App {
    pub fn new(records: Vec<Record>, output_dir: PathBuf) -> Self {
        Self {
            screen: Screen::List,
            help_return: Screen::List,
            records,
            filter: String::new(),
            filtering: false,
            selected: 0,
            form: NameForm {
                name: String::new(),
                mode: CaptureMode::Mixed,
            },
            recording: None,
            modal: None,
            status: None,
            preview: None,
            output_dir,
        }
    }

    /// Records matching the current filter (case-insensitive substring of the
    /// directory name), in scan order (newest first).
    pub fn visible(&self) -> Vec<&Record> {
        let needle = self.filter.to_lowercase();
        self.records
            .iter()
            .filter(|r| needle.is_empty() || r.dir_name.to_lowercase().contains(&needle))
            .collect()
    }

    pub fn selected_record(&self) -> Option<&Record> {
        self.visible().get(self.selected).copied()
    }

    fn clamp_selection(&mut self) {
        let len = self.visible().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    fn set_status(&mut self, text: impl Into<String>, is_error: bool) {
        self.status = Some(Status {
            text: text.into(),
            is_error,
        });
    }

    // ------------------------------------------------------------------ input

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        self.status = None;

        // Ctrl+C: stop & save while recording, quit everywhere else.
        if ctrl && key.code == KeyCode::Char('c') {
            self.modal = None;
            return if self.screen == Screen::Recording {
                vec![Effect::StopRecording { discard: false }]
            } else {
                vec![Effect::Quit]
            };
        }

        if let Some(modal) = self.modal.clone() {
            return self.handle_modal_key(modal, key);
        }

        match self.screen {
            Screen::List => self.handle_list_key(key),
            Screen::NameForm => self.handle_form_key(key),
            Screen::Recording => self.handle_recording_key(key),
            Screen::Detail => self.handle_detail_key(key),
            Screen::Help => {
                self.screen = self.help_return;
                Vec::new()
            }
        }
    }

    fn open_help(&mut self) {
        self.help_return = self.screen;
        self.screen = Screen::Help;
    }

    fn handle_modal_key(&mut self, modal: Modal, key: KeyEvent) -> Vec<Effect> {
        // Only an explicit `y` confirms. Enter deliberately does not, so a stray
        // keypress can't delete or discard a recording.
        let confirmed = matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y'));
        self.modal = None;
        if !confirmed {
            return Vec::new();
        }
        match modal {
            Modal::ConfirmDiscard => vec![Effect::StopRecording { discard: true }],
            Modal::ConfirmDelete { dir_name } => match self.record_by_name(&dir_name) {
                Some(path) => vec![Effect::DeleteRecord { path, dir_name }],
                None => Vec::new(),
            },
        }
    }

    fn record_by_name(&self, dir_name: &str) -> Option<PathBuf> {
        self.records
            .iter()
            .find(|r| r.dir_name == dir_name)
            .map(|r| r.path.clone())
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if self.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filtering = false;
                }
                KeyCode::Enter => self.filtering = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::ALT) => {
                    self.filter.push(c);
                }
                _ => {}
            }
            self.selected = 0;
            self.clamp_selection();
            return Vec::new();
        }

        let len = self.visible().len();
        match key.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.clamp_selection();
                } else {
                    return vec![Effect::Quit];
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if len > 0 {
                    self.selected = (self.selected + 1).min(len - 1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.selected = len.saturating_sub(1),
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('?') => self.open_help(),
            KeyCode::Char('r') => {
                self.form.name.clear();
                self.screen = Screen::NameForm;
            }
            KeyCode::Enter => {
                if let Some(record) = self.selected_record() {
                    let path = record.path.clone();
                    let has_transcript = record.has_transcript;
                    self.preview = None;
                    self.screen = Screen::Detail;
                    if has_transcript {
                        return vec![Effect::LoadPreview { path }];
                    }
                }
            }
            KeyCode::Char('o') => return self.reveal_selected(),
            KeyCode::Char('d') => self.ask_delete_selected(),
            KeyCode::Char('t') => self.not_implemented_transcribe(),
            _ => {}
        }
        Vec::new()
    }

    fn reveal_selected(&mut self) -> Vec<Effect> {
        match self.selected_record() {
            Some(r) => vec![Effect::RevealRecord {
                path: r.path.clone(),
            }],
            None => Vec::new(),
        }
    }

    fn ask_delete_selected(&mut self) {
        if let Some(r) = self.selected_record() {
            self.modal = Some(Modal::ConfirmDelete {
                dir_name: r.dir_name.clone(),
            });
        }
    }

    fn not_implemented_transcribe(&mut self) {
        self.set_status(
            "Transcription isn't implemented yet in this port (planned: Phase 7).",
            false,
        );
    }

    fn handle_form_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc => self.screen = Screen::List,
            KeyCode::Tab => self.form.mode = next_mode(self.form.mode),
            KeyCode::Backspace => {
                self.form.name.pop();
            }
            KeyCode::Enter => {
                let name = self.form.name.trim();
                return vec![Effect::StartRecording {
                    name: (!name.is_empty()).then(|| name.to_string()),
                    mode: self.form.mode,
                }];
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::ALT) => {
                self.form.name.push(c);
            }
            _ => {}
        }
        Vec::new()
    }

    fn handle_recording_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('s') | KeyCode::Char('q') => {
                vec![Effect::StopRecording { discard: false }]
            }
            KeyCode::Char('x') => {
                self.modal = Some(Modal::ConfirmDiscard);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_detail_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => {
                self.screen = Screen::List;
                self.preview = None;
            }
            KeyCode::Char('o') => return self.reveal_selected(),
            KeyCode::Char('d') => self.ask_delete_selected(),
            KeyCode::Char('t') => self.not_implemented_transcribe(),
            KeyCode::Char('?') => self.open_help(),
            _ => {}
        }
        Vec::new()
    }

    // ------------------------------------------------- results from the event loop

    pub fn on_records(&mut self, records: Vec<Record>) {
        let keep = self.selected_record().map(|r| r.dir_name.clone());
        self.records = records;
        if let Some(name) = keep {
            if let Some(i) = self.visible().iter().position(|r| r.dir_name == name) {
                self.selected = i;
            }
        }
        self.clamp_selection();
    }

    pub fn on_recording_started(
        &mut self,
        dir_name: &str,
        mode: CaptureMode,
        warning: Option<String>,
    ) {
        self.recording = Some(RecordingView {
            dir_name: dir_name.to_string(),
            mode,
            elapsed: Duration::ZERO,
            sys_level: 0.0,
            mic_level: 0.0,
            warning,
        });
        self.screen = Screen::Recording;
    }

    pub fn on_start_failed(&mut self, message: &str) {
        self.screen = Screen::List;
        self.set_status(format!("Couldn't start recording: {message}"), true);
    }

    pub fn on_recording_tick(&mut self, elapsed: Duration, levels: Option<LevelReading>) {
        if let Some(view) = self.recording.as_mut() {
            view.elapsed = elapsed;
            if let Some(l) = levels {
                view.sys_level = l.sys.unwrap_or(0.0);
                view.mic_level = l.mic.unwrap_or(0.0);
            }
        }
    }

    pub fn on_recording_ended(&mut self, outcome: Outcome, dir_name: &str, message: &str) {
        self.recording = None;
        self.modal = None;
        self.screen = Screen::List;
        match outcome {
            Outcome::Saved => self.set_status(format!("Saved {dir_name}"), false),
            Outcome::DiscardedTooShort => {
                self.set_status("Recording was under 2 seconds - discarded.", false)
            }
            Outcome::DiscardedByUser => self.set_status("Recording discarded.", false),
            Outcome::FailedNoAudio => self.set_status(message.to_string(), true),
        }
    }

    pub fn on_preview(&mut self, preview: Option<String>) {
        self.preview = preview;
    }

    pub fn on_deleted(&mut self, dir_name: &str) {
        self.screen = Screen::List;
        self.preview = None;
        self.set_status(
            format!("Moved {dir_name} to the Recycle Bin / Trash."),
            false,
        );
    }

    pub fn on_action_error(&mut self, message: &str) {
        self.set_status(message.to_string(), true);
    }

    pub fn show_busy(&mut self, message: &str) {
        self.set_status(message.to_string(), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nastro_records::parse_dir_name;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn rec(name: &str, transcript: bool) -> Record {
        Record {
            dir_name: name.to_string(),
            path: PathBuf::from("/rec").join(name),
            parsed: parse_dir_name(name).unwrap(),
            size_bytes: 1024,
            duration_seconds: Some(65.0),
            has_transcript: transcript,
        }
    }

    fn app() -> App {
        App::new(
            vec![
                rec("2026-10-22-1430-standup", true),
                rec("2026-10-21-1000-planning", false),
                rec("2026-10-20-0900", false),
            ],
            PathBuf::from("/rec"),
        )
    }

    // ----- list navigation

    #[test]
    fn down_and_up_move_selection_within_bounds() {
        let mut a = app();
        a.handle_key(ch('j'));
        a.handle_key(key(KeyCode::Down));
        a.handle_key(ch('j')); // already last
        assert_eq!(a.selected, 2);
        a.handle_key(ch('k'));
        assert_eq!(a.selected, 1);
        a.handle_key(key(KeyCode::Up));
        a.handle_key(key(KeyCode::Up)); // already first
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn home_and_end_jump() {
        let mut a = app();
        a.handle_key(ch('G'));
        assert_eq!(a.selected, 2);
        a.handle_key(ch('g'));
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn navigation_on_an_empty_list_does_not_panic() {
        let mut a = App::new(vec![], PathBuf::from("/rec"));
        for k in ['j', 'k', 'G', 'g', 'o', 'd'] {
            assert!(a.handle_key(ch(k)).is_empty());
        }
        assert!(a.handle_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(a.screen, Screen::List);
    }

    #[test]
    fn q_quits_from_the_list() {
        assert_eq!(app().handle_key(ch('q')), vec![Effect::Quit]);
    }

    #[test]
    fn ctrl_c_quits_from_the_list() {
        assert_eq!(app().handle_key(ctrl('c')), vec![Effect::Quit]);
    }

    #[test]
    fn key_release_events_are_ignored() {
        let mut a = app();
        let mut release = ch('q');
        release.kind = KeyEventKind::Release;
        assert!(a.handle_key(release).is_empty());
    }

    // ----- filter

    #[test]
    fn filter_narrows_visible_records_case_insensitively() {
        let mut a = app();
        a.handle_key(ch('/'));
        for c in "PLAN".chars() {
            a.handle_key(ch(c));
        }
        let names: Vec<_> = a.visible().iter().map(|r| r.dir_name.as_str()).collect();
        assert_eq!(names, vec!["2026-10-21-1000-planning"]);
        assert!(a.filtering);
    }

    #[test]
    fn typing_command_letters_while_filtering_does_not_trigger_commands() {
        let mut a = app();
        a.handle_key(ch('/'));
        assert!(a.handle_key(ch('q')).is_empty(), "q must type, not quit");
        assert!(a.handle_key(ch('d')).is_empty(), "d must type, not delete");
        assert_eq!(a.filter, "qd");
        assert!(a.modal.is_none());
    }

    #[test]
    fn enter_keeps_the_filter_and_esc_clears_it() {
        let mut a = app();
        a.handle_key(ch('/'));
        a.handle_key(ch('s'));
        a.handle_key(key(KeyCode::Enter));
        assert!(!a.filtering);
        assert_eq!(a.filter, "s");

        // Esc on the list clears an active filter instead of quitting.
        assert!(a.handle_key(key(KeyCode::Esc)).is_empty());
        assert_eq!(a.filter, "");
        // ...and only then does Esc quit.
        assert_eq!(a.handle_key(key(KeyCode::Esc)), vec![Effect::Quit]);
    }

    #[test]
    fn esc_while_typing_a_filter_cancels_it() {
        let mut a = app();
        a.handle_key(ch('/'));
        a.handle_key(ch('x'));
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.filter, "");
        assert!(!a.filtering);
        assert_eq!(a.visible().len(), 3);
    }

    #[test]
    fn backspace_edits_the_filter() {
        let mut a = app();
        a.handle_key(ch('/'));
        a.handle_key(ch('a'));
        a.handle_key(ch('b'));
        a.handle_key(key(KeyCode::Backspace));
        assert_eq!(a.filter, "a");
    }

    #[test]
    fn selection_resets_and_clamps_when_the_filter_shrinks_the_list() {
        let mut a = app();
        a.handle_key(ch('G'));
        a.handle_key(ch('/'));
        a.handle_key(ch('s')); // matches only "standup"
        assert_eq!(a.visible().len(), 1);
        assert_eq!(a.selected, 0);
    }

    // ----- record flow

    #[test]
    fn r_opens_the_name_form_with_an_empty_name() {
        let mut a = app();
        a.form.name = "stale".into();
        a.handle_key(ch('r'));
        assert_eq!(a.screen, Screen::NameForm);
        assert_eq!(a.form.name, "");
    }

    #[test]
    fn form_collects_a_name_and_cycles_modes_with_tab() {
        let mut a = app();
        a.handle_key(ch('r'));
        for c in "Weekly Sync".chars() {
            a.handle_key(ch(c));
        }
        a.handle_key(key(KeyCode::Tab));
        assert_eq!(a.form.mode, CaptureMode::MicOnly);
        a.handle_key(key(KeyCode::Tab));
        assert_eq!(a.form.mode, CaptureMode::SystemOnly);
        a.handle_key(key(KeyCode::Tab));
        assert_eq!(a.form.mode, CaptureMode::Mixed);

        let fx = a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            fx,
            vec![Effect::StartRecording {
                name: Some("Weekly Sync".into()),
                mode: CaptureMode::Mixed
            }]
        );
    }

    #[test]
    fn empty_or_blank_name_starts_an_unnamed_recording() {
        let mut a = app();
        a.handle_key(ch('r'));
        a.handle_key(ch(' '));
        let fx = a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            fx,
            vec![Effect::StartRecording {
                name: None,
                mode: CaptureMode::Mixed
            }]
        );
    }

    #[test]
    fn esc_cancels_the_form() {
        let mut a = app();
        a.handle_key(ch('r'));
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.screen, Screen::List);
    }

    #[test]
    fn q_is_text_in_the_form_not_quit() {
        let mut a = app();
        a.handle_key(ch('r'));
        assert!(a.handle_key(ch('q')).is_empty());
        assert_eq!(a.form.name, "q");
    }

    #[test]
    fn started_switches_to_the_recording_screen() {
        let mut a = app();
        a.on_recording_started("2026-10-22-1500", CaptureMode::MicOnly, Some("low".into()));
        assert_eq!(a.screen, Screen::Recording);
        let v = a.recording.as_ref().unwrap();
        assert_eq!(v.mode, CaptureMode::MicOnly);
        assert_eq!(v.warning.as_deref(), Some("low"));
    }

    #[test]
    fn start_failure_returns_to_the_list_with_an_error() {
        let mut a = app();
        a.screen = Screen::NameForm;
        a.on_start_failed("ffmpeg missing");
        assert_eq!(a.screen, Screen::List);
        let s = a.status.as_ref().unwrap();
        assert!(s.is_error && s.text.contains("ffmpeg missing"));
    }

    fn recording_app() -> App {
        let mut a = app();
        a.on_recording_started("2026-10-22-1500", CaptureMode::Mixed, None);
        a
    }

    #[test]
    fn s_q_and_ctrl_c_all_stop_and_save_while_recording() {
        for k in [ch('s'), ch('q'), ctrl('c')] {
            assert_eq!(
                recording_app().handle_key(k),
                vec![Effect::StopRecording { discard: false }],
                "{k:?}"
            );
        }
    }

    #[test]
    fn discard_requires_explicit_y_confirmation() {
        let mut a = recording_app();
        assert!(a.handle_key(ch('x')).is_empty());
        assert_eq!(a.modal, Some(Modal::ConfirmDiscard));

        // Enter does NOT confirm.
        assert!(a.handle_key(key(KeyCode::Enter)).is_empty());
        assert!(a.modal.is_none());

        a.handle_key(ch('x'));
        assert_eq!(
            a.handle_key(ch('y')),
            vec![Effect::StopRecording { discard: true }]
        );
    }

    #[test]
    fn declining_discard_keeps_recording() {
        let mut a = recording_app();
        a.handle_key(ch('x'));
        assert!(a.handle_key(ch('n')).is_empty());
        assert!(a.modal.is_none());
        assert_eq!(a.screen, Screen::Recording);
    }

    #[test]
    fn ctrl_c_during_the_discard_prompt_still_saves() {
        let mut a = recording_app();
        a.handle_key(ch('x'));
        assert_eq!(
            a.handle_key(ctrl('c')),
            vec![Effect::StopRecording { discard: false }]
        );
        assert!(a.modal.is_none());
    }

    #[test]
    fn stray_keys_while_recording_do_nothing() {
        let mut a = recording_app();
        for c in ['d', 'r', 'o', 't', 'j'] {
            assert!(a.handle_key(ch(c)).is_empty());
        }
        assert_eq!(a.screen, Screen::Recording);
    }

    #[test]
    fn ticks_update_elapsed_and_levels() {
        let mut a = recording_app();
        a.on_recording_tick(
            Duration::from_secs(5),
            Some(LevelReading {
                sys: Some(0.4),
                mic: None,
            }),
        );
        let v = a.recording.as_ref().unwrap();
        assert_eq!(v.elapsed, Duration::from_secs(5));
        assert_eq!(v.sys_level, 0.4);
        assert_eq!(v.mic_level, 0.0);

        // A tick with no new reading keeps the last levels.
        a.on_recording_tick(Duration::from_secs(6), None);
        assert_eq!(a.recording.as_ref().unwrap().sys_level, 0.4);
    }

    #[test]
    fn ended_outcomes_set_the_right_status() {
        let cases = [
            (Outcome::Saved, false, "Saved"),
            (Outcome::DiscardedTooShort, false, "under 2 seconds"),
            (Outcome::DiscardedByUser, false, "discarded"),
            (Outcome::FailedNoAudio, true, "boom"),
        ];
        for (outcome, is_error, needle) in cases {
            let mut a = recording_app();
            a.on_recording_ended(outcome, "2026-10-22-1500", "boom");
            assert_eq!(a.screen, Screen::List);
            assert!(a.recording.is_none());
            let s = a.status.as_ref().unwrap();
            assert_eq!(s.is_error, is_error, "{outcome:?}");
            assert!(s.text.contains(needle), "{outcome:?}: {}", s.text);
        }
    }

    // ----- detail

    #[test]
    fn enter_opens_detail_and_requests_a_preview_only_when_a_transcript_exists() {
        let mut a = app();
        let fx = a.handle_key(key(KeyCode::Enter));
        assert_eq!(a.screen, Screen::Detail);
        assert_eq!(
            fx,
            vec![Effect::LoadPreview {
                path: PathBuf::from("/rec/2026-10-22-1430-standup")
            }]
        );

        a.handle_key(key(KeyCode::Esc));
        a.handle_key(ch('j')); // "planning" has no transcript
        assert!(a.handle_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(a.screen, Screen::Detail);
    }

    #[test]
    fn leaving_detail_clears_the_preview() {
        let mut a = app();
        a.handle_key(key(KeyCode::Enter));
        a.on_preview(Some("hello".into()));
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.screen, Screen::List);
        assert!(a.preview.is_none());
    }

    #[test]
    fn detail_supports_reveal_and_delete() {
        let mut a = app();
        a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            a.handle_key(ch('o')),
            vec![Effect::RevealRecord {
                path: PathBuf::from("/rec/2026-10-22-1430-standup")
            }]
        );
        a.handle_key(ch('d'));
        assert!(matches!(a.modal, Some(Modal::ConfirmDelete { .. })));
    }

    // ----- delete

    #[test]
    fn delete_needs_y_and_targets_the_selected_record() {
        let mut a = app();
        a.handle_key(ch('j')); // planning
        a.handle_key(ch('d'));
        assert_eq!(
            a.modal,
            Some(Modal::ConfirmDelete {
                dir_name: "2026-10-21-1000-planning".into()
            })
        );
        let fx = a.handle_key(ch('y'));
        assert_eq!(
            fx,
            vec![Effect::DeleteRecord {
                path: PathBuf::from("/rec/2026-10-21-1000-planning"),
                dir_name: "2026-10-21-1000-planning".into()
            }]
        );
    }

    #[test]
    fn delete_declined_by_any_other_key() {
        for k in [ch('n'), key(KeyCode::Esc), key(KeyCode::Enter), ch('d')] {
            let mut a = app();
            a.handle_key(ch('d'));
            assert!(a.handle_key(k).is_empty());
            assert!(a.modal.is_none());
        }
    }

    #[test]
    fn deleted_returns_to_the_list_with_a_status() {
        let mut a = app();
        a.screen = Screen::Detail;
        a.on_deleted("x");
        assert_eq!(a.screen, Screen::List);
        assert!(a.status.as_ref().unwrap().text.contains("Recycle Bin"));
    }

    // ----- help / transcribe / status

    #[test]
    fn help_returns_to_the_screen_it_was_opened_from() {
        let mut a = app();
        a.handle_key(ch('?'));
        assert_eq!(a.screen, Screen::Help);
        a.handle_key(ch('z'));
        assert_eq!(a.screen, Screen::List);

        a.handle_key(key(KeyCode::Enter));
        a.handle_key(ch('?'));
        a.handle_key(ch('z'));
        assert_eq!(a.screen, Screen::Detail);
    }

    #[test]
    fn transcribe_reports_not_implemented_instead_of_silently_doing_nothing() {
        let mut a = app();
        assert!(a.handle_key(ch('t')).is_empty());
        assert!(a.status.as_ref().unwrap().text.contains("Phase 7"));
    }

    #[test]
    fn status_clears_on_the_next_key() {
        let mut a = app();
        a.handle_key(ch('t'));
        assert!(a.status.is_some());
        a.handle_key(ch('j'));
        assert!(a.status.is_none());
    }

    #[test]
    fn rescan_keeps_the_selected_record_selected_when_it_moves() {
        let mut a = app();
        a.handle_key(ch('j')); // planning
        let mut fresh = vec![rec("2026-10-22-1500-new", false)];
        fresh.extend(a.records.clone());
        a.on_records(fresh);
        assert_eq!(
            a.selected_record().unwrap().dir_name,
            "2026-10-21-1000-planning"
        );
    }

    #[test]
    fn rescan_that_removes_the_selection_clamps_it() {
        let mut a = app();
        a.handle_key(ch('G'));
        a.on_records(vec![rec("2026-10-22-1430-standup", false)]);
        assert_eq!(a.selected, 0);
        a.on_records(vec![]);
        assert_eq!(a.selected, 0);
        assert!(a.selected_record().is_none());
    }

    #[test]
    fn next_mode_visits_every_mode_then_wraps() {
        let start = CaptureMode::Mixed;
        let a = next_mode(start);
        let b = next_mode(a);
        let c = next_mode(b);
        assert_eq!(c, start, "three steps should wrap back to the start");
        assert!(a != start && b != start && a != b, "all three must differ");
    }
}

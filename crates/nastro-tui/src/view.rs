//! Rendering. Only ANSI palette colors are used so the UI inherits the user's
//! terminal theme.
//!
//! Note on OSC 8 hyperlinks: the original project wraps file paths in OSC 8
//! escapes so they are clickable. ratatui draws into a cell buffer and counts every
//! character of an escape sequence as a visible cell, which corrupts layout, so
//! paths are rendered as plain text here instead.

use std::time::Duration;

use nastro_record::CaptureMode;
use nastro_records::{format_duration, format_size, transcript_preview};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use crate::app::{mode_label, App, Modal, Screen};

const PREVIEW_CHARS: usize = 600;
const METER_WIDTH: usize = 24;
/// Quietest level shown on the meter; anything below reads as empty.
const METER_FLOOR_DB: f32 = -60.0;

const DIM: Style = Style::new().fg(Color::DarkGray);
const HINT_KEY: Style = Style::new().add_modifier(Modifier::BOLD);

/// `m:ss`, or `h:mm:ss` from an hour up.
pub fn format_timer(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Map an RMS level in `[0, 1]` to a `[0, 1]` meter fraction on a dB scale.
/// Linear RMS makes ordinary speech (~0.05) look almost empty; dB matches how
/// loudness is perceived.
pub fn level_fraction(rms: f32) -> f32 {
    if rms <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * rms.min(1.0).log10();
    ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0)
}

pub fn level_bar(rms: f32, width: usize) -> String {
    let filled = (level_fraction(rms) * width as f32).round() as usize;
    let filled = filled.min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [col] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(row);
    col
}

pub fn draw(frame: &mut Frame, app: &App) {
    let [title, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(Paragraph::new(title_line(app)), title);

    match app.screen {
        Screen::List => draw_list(frame, app, body),
        Screen::NameForm => {
            draw_list(frame, app, body);
            draw_form(frame, app, body);
        }
        Screen::Recording => draw_recording(frame, app, body),
        Screen::Detail => draw_detail(frame, app, body),
        Screen::Help => draw_help(frame, body),
    }

    frame.render_widget(Paragraph::new(footer_line(app, footer.width)), footer);

    if let Some(modal) = &app.modal {
        draw_modal(frame, modal, body);
    }
}

fn title_line(app: &App) -> Line<'static> {
    let mut spans = vec![Span::styled(
        " nastro ",
        Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD),
    )];
    match app.screen {
        Screen::Recording => spans.push(Span::styled(
            "  ● REC",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        _ => {
            let n = app.records.len();
            spans.push(Span::raw(format!(
                "  {n} recording{}",
                if n == 1 { "" } else { "s" }
            )));
            if !app.filter.is_empty() {
                spans.push(Span::styled(format!("  filter: {}", app.filter), DIM));
            }
        }
    }
    Line::from(spans)
}

/// Like [`hint`], but on a narrow terminal drops the hints whose keys are in `droppable` (in that order) until the
/// bar fits, so the ones that matter most (help, quit) are never the ones cut off at the edge.
fn hint_fit(pairs: &[(&str, &str)], droppable: &[&str], width: u16) -> Line<'static> {
    let needed = |p: &[(&str, &str)]| -> usize {
        1 + p
            .iter()
            .map(|(k, w)| k.chars().count() + 1 + w.chars().count())
            .sum::<usize>()
            + 5 * p.len().saturating_sub(1)
    };
    let mut pairs = pairs.to_vec();
    for key in droppable {
        if needed(&pairs) <= usize::from(width) {
            break;
        }
        pairs.retain(|(k, _)| k != key);
    }
    hint(&pairs)
}

fn hint(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, what)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", DIM));
        }
        spans.push(Span::styled(key.to_string(), HINT_KEY));
        spans.push(Span::styled(format!(" {what}"), DIM));
    }
    Line::from(spans)
}

fn footer_line(app: &App, width: u16) -> Line<'static> {
    if let Some(status) = &app.status {
        let style = if status.is_error {
            Style::new().fg(Color::Red)
        } else {
            Style::new().fg(Color::Green)
        };
        return Line::from(Span::styled(format!(" {}", status.text), style));
    }
    if app.modal.is_some() {
        return hint(&[("y", "confirm"), ("any other key", "cancel")]);
    }
    if app.screen == Screen::List && app.filtering {
        return Line::from(vec![
            Span::styled(" /", HINT_KEY),
            Span::raw(format!("{}▏", app.filter)),
            Span::styled("   enter apply · esc clear", DIM),
        ]);
    }
    match app.screen {
        Screen::List => hint_fit(
            &[
                ("r", "record"),
                ("enter", "open"),
                ("/", "filter"),
                ("t", "transcribe"),
                ("o", "reveal"),
                ("d", "delete"),
                ("?", "help"),
                ("q", "quit"),
            ],
            &["o", "d", "/", "enter", "r", "t"],
            width,
        ),
        Screen::NameForm => hint(&[("enter", "start"), ("tab", "mode"), ("esc", "cancel")]),
        Screen::Recording => hint(&[("s", "stop & save"), ("x", "discard")]),
        Screen::Detail => hint_fit(
            &[
                ("esc", "back"),
                ("t", "transcribe"),
                ("o", "reveal"),
                ("d", "delete"),
                ("?", "help"),
            ],
            &["o", "d", "esc", "t"],
            width,
        ),
        Screen::Help => hint(&[("any key", "close")]),
    }
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let visible = app.visible();

    if visible.is_empty() {
        let msg = if app.records.is_empty() {
            format!(
                "No recordings yet in {}\n\nPress r to record.",
                app.output_dir.display()
            )
        } else {
            "No recordings match the filter.".to_string()
        };
        frame.render_widget(
            Paragraph::new(msg).style(DIM).wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let rows = visible.iter().map(|r| {
        let dur = r
            .duration_seconds
            .map(format_duration)
            .unwrap_or_else(|| "-".into());
        Row::new(vec![
            r.dir_name.clone(),
            dur,
            format_size(r.size_bytes),
            match app.jobs.get(&r.dir_name) {
                Some(job) => job.short(),
                None if r.has_transcript => "✓".to_string(),
                None => String::new(),
            },
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(9),
        ],
    )
    .header(Row::new(vec!["NAME", "LENGTH", "SIZE", "T"]).style(DIM))
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    let mut state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_form(frame: &mut Frame, app: &App, area: Rect) {
    let rect = centered(58, 10, area);
    frame.render_widget(Clear, rect);

    let radio = |m: CaptureMode| {
        let on = app.form.mode == m;
        Span::styled(
            format!("{} {}   ", if on { "◉" } else { "○" }, mode_label(m)),
            if on {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                DIM
            },
        )
    };

    let text = Text::from(vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  Name  ", DIM),
            Span::raw(format!("{}▏", app.form.name)),
        ]),
        Line::raw(""),
        Line::from(vec![Span::styled("  Mode  ", DIM)]),
        Line::from(vec![
            Span::raw("  "),
            radio(CaptureMode::Mixed),
            radio(CaptureMode::MicOnly),
        ]),
        Line::from(vec![Span::raw("  "), radio(CaptureMode::SystemOnly)]),
        Line::raw(""),
        Line::styled("  Leave the name empty for an unnamed recording.", DIM),
    ]);
    frame.render_widget(
        Paragraph::new(text).block(Block::bordered().title(" New recording ")),
        rect,
    );
}

fn draw_recording(frame: &mut Frame, app: &App, area: Rect) {
    let Some(view) = &app.recording else {
        return;
    };
    let rect = centered(56, 13, area);

    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("● {}", format_timer(view.elapsed)),
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))
        .centered(),
        Line::from(Span::styled(view.dir_name.clone(), DIM)).centered(),
        Line::raw(""),
    ];

    let wants_sys = view.mode != CaptureMode::MicOnly;
    let wants_mic = view.mode != CaptureMode::SystemOnly;
    let meter = |label: &str, active: bool, level: f32| {
        if active {
            Line::from(vec![
                Span::styled(format!("  {label}  "), DIM),
                Span::raw(level_bar(level, METER_WIDTH)),
            ])
        } else {
            Line::from(Span::styled(format!("  {label}  (off)"), DIM))
        }
    };
    lines.push(meter("system", wants_sys, view.sys_level));
    lines.push(meter("mic   ", wants_mic, view.mic_level));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(mode_label(view.mode), DIM)).centered());
    if let Some(w) = &view.warning {
        lines.push(Line::from(Span::styled(
            w.clone(),
            Style::new().fg(Color::Yellow),
        )));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::bordered().title(" Recording "))
            .wrap(Wrap { trim: true }),
        rect,
    );
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.selected_record() else {
        frame.render_widget(
            Paragraph::new("Recording no longer exists.").style(DIM),
            area,
        );
        return;
    };

    let kv =
        |k: &str, v: String| Line::from(vec![Span::styled(format!("{k:<11}"), DIM), Span::raw(v)]);
    let mut lines = vec![
        kv("Name", r.dir_name.clone()),
        kv("Folder", r.path.display().to_string()),
        kv(
            "Length",
            r.duration_seconds
                .map(format_duration)
                .unwrap_or_else(|| "unknown".into()),
        ),
        kv("Size", format_size(r.size_bytes)),
        kv(
            "Transcript",
            match app.jobs.get(&r.dir_name) {
                Some(job) => format!("{} - c to cancel", job.describe()),
                None if r.has_transcript => "yes".to_string(),
                None => "none yet (t to transcribe)".to_string(),
            },
        ),
        Line::raw(""),
    ];
    if let Some(p) = &app.preview {
        lines.push(Line::styled("Transcript preview", DIM));
        lines.push(Line::raw(transcript_preview(p, PREVIEW_CHARS)));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::bordered().title(" Recording "))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let rows = [
        ("r", "record (name + capture mode)"),
        ("s / q / ctrl+c", "stop & save (while recording)"),
        ("x", "discard recording (asks first)"),
        ("enter", "open recording details"),
        (
            "t",
            "transcribe in the background (asks before downloading a model)",
        ),
        ("c", "cancel the selected recording's transcription"),
        ("o", "reveal in file manager"),
        ("d", "delete -> Recycle Bin / Trash (asks first)"),
        ("/", "filter by name"),
        ("j k / arrows", "move"),
        ("g G", "first / last"),
        ("?", "this help"),
        ("q", "quit"),
    ];
    let mut lines = vec![Line::raw("")];
    for (k, v) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<16}"), HINT_KEY),
            Span::raw(v),
        ]));
    }
    let rect = centered(64, rows.len() as u16 + 3, area);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::bordered().title(" Keys ")),
        rect,
    );
}

fn draw_modal(frame: &mut Frame, modal: &Modal, area: Rect) {
    let (title, body) = match modal {
        Modal::ConfirmDelete { dir_name } => (
            " Delete recording? ",
            format!("Move {dir_name} to the Recycle Bin / Trash?"),
        ),
        Modal::ConfirmDiscard => (
            " Discard recording? ",
            "Stop and delete this recording?".to_string(),
        ),
        Modal::ConfirmOverwriteTranscript { dir_name } => (
            " Replace transcript? ",
            format!("{dir_name} is already transcribed. Transcribe again and replace it?"),
        ),
        Modal::ConfirmDownloadModel { model, size_text, .. } => (
            " Download speech model? ",
            format!("The model {model} ({size_text}) is not on this computer yet. Download it from huggingface.co?"),
        ),
        Modal::ConfirmQuitWithJobs => (
            " Quit? ",
            "Transcription is still running. Quit and cancel it?".to_string(),
        ),
    };
    let rect = centered(60, 6, area);
    frame.render_widget(Clear, rect);
    let text = Text::from(vec![
        Line::raw(""),
        Line::raw(format!("  {body}")),
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("y", HINT_KEY),
            Span::styled(" yes   ", DIM),
            Span::styled("any other key", HINT_KEY),
            Span::styled(" cancel", DIM),
        ]),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::bordered()
                    .border_style(Style::new().fg(Color::Yellow))
                    .title(title),
            )
            .wrap(Wrap { trim: true }),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use nastro_record::{LevelReading, Outcome};
    use nastro_records::{parse_dir_name, Record};
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn rec(name: &str, transcript: bool) -> Record {
        Record {
            dir_name: name.into(),
            path: PathBuf::from("/rec").join(name),
            parsed: parse_dir_name(name).unwrap(),
            size_bytes: 2048,
            duration_seconds: Some(125.0),
            has_transcript: transcript,
        }
    }

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn app() -> App {
        App::new(
            vec![
                rec("2026-10-22-1430-standup", true),
                rec("2026-10-21-1000-planning", false),
            ],
            PathBuf::from("/rec"),
        )
    }

    // ----- pure helpers

    #[test]
    fn timer_formats_minutes_and_hours() {
        assert_eq!(format_timer(Duration::from_secs(0)), "0:00");
        assert_eq!(format_timer(Duration::from_secs(65)), "1:05");
        assert_eq!(format_timer(Duration::from_secs(3600)), "1:00:00");
        assert_eq!(format_timer(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn level_fraction_is_zero_for_silence_and_one_for_full_scale() {
        assert_eq!(level_fraction(0.0), 0.0);
        assert_eq!(level_fraction(-1.0), 0.0);
        assert!((level_fraction(1.0) - 1.0).abs() < 1e-6);
        assert!((level_fraction(5.0) - 1.0).abs() < 1e-6, "clamps above 1");
    }

    #[test]
    fn level_fraction_is_monotonic_and_shows_quiet_speech() {
        assert!(level_fraction(0.01) < level_fraction(0.05));
        assert!(level_fraction(0.05) < level_fraction(0.5));
        // Ordinary speech (~0.05 RMS, about -26 dB) should be visibly non-empty on a
        // dB scale - the reason a linear scale was rejected.
        assert!(level_fraction(0.05) > 0.4);
    }

    #[test]
    fn level_bar_has_constant_width() {
        for lvl in [0.0, 0.001, 0.05, 0.5, 1.0] {
            assert_eq!(level_bar(lvl, 10).chars().count(), 10);
        }
        assert_eq!(level_bar(0.0, 4), "░░░░");
        assert_eq!(level_bar(1.0, 4), "████");
    }

    // ----- rendered screens

    #[test]
    fn list_shows_records_columns_and_hints() {
        let out = render(&app(), 90, 12);
        assert!(out.contains("2 recordings"));
        assert!(out.contains("2026-10-22-1430-standup"));
        assert!(out.contains("2:05"), "duration column");
        assert!(out.contains("2.0 KB"), "size column");
        assert!(out.contains('✓'), "transcript flag");
        assert!(out.contains("record") && out.contains("quit"));
    }

    #[test]
    fn the_hint_bar_always_keeps_help_and_quit_however_narrow_the_terminal() {
        for width in [40u16, 60, 70, 80, 100, 120] {
            let out = render(&app(), width, 10);
            let last = out.lines().last().unwrap_or("");
            assert!(
                last.contains("help") && last.contains("quit"),
                "width {width}: {last:?}"
            );
            assert!(
                last.contains("transcribe") || width < 50,
                "width {width}: {last:?}"
            );
        }
    }

    #[test]
    fn a_wide_terminal_shows_every_hint() {
        let out = render(&app(), 120, 10);
        let last = out.lines().last().unwrap_or("");
        for want in [
            "record",
            "open",
            "filter",
            "transcribe",
            "reveal",
            "delete",
            "help",
            "quit",
        ] {
            assert!(last.contains(want), "{want} missing: {last:?}");
        }
    }

    #[test]
    fn a_running_job_shows_in_the_list_and_the_detail_view() {
        let mut a = app();
        a.on_transcribe_state(
            "2026-10-22-1430-standup",
            crate::app::JobState::Transcribing(37),
        );
        let list = render(&a, 100, 12);
        assert!(list.contains("37%"), "{list}");

        a.screen = Screen::Detail;
        let detail = render(&a, 100, 12);
        assert!(
            detail.contains("transcribing (37%)") && detail.contains("c to cancel"),
            "{detail}"
        );
    }

    #[test]
    fn the_new_dialogs_render_their_question() {
        let mut a = app();
        a.modal = Some(Modal::ConfirmOverwriteTranscript {
            dir_name: "2026-10-22-1430-standup".into(),
        });
        assert!(render(&a, 100, 14).contains("replace it"));
        a.modal = Some(Modal::ConfirmDownloadModel {
            dir_name: "x".into(),
            model: "large-v3-turbo".into(),
            size_text: "1.5 GiB".into(),
            overwrite: false,
        });
        let out = render(&a, 100, 14);
        assert!(
            out.contains("large-v3-turbo")
                && out.contains("1.5 GiB")
                && out.contains("huggingface.co"),
            "{out}"
        );
        a.modal = Some(Modal::ConfirmQuitWithJobs);
        assert!(render(&a, 100, 14).contains("cancel it"));
    }

    #[test]
    fn empty_list_explains_where_recordings_go() {
        let a = App::new(vec![], PathBuf::from("/some/dir"));
        let out = render(&a, 80, 10);
        assert!(out.contains("No recordings yet"));
        assert!(out.contains("/some/dir"));
    }

    #[test]
    fn filter_with_no_matches_says_so() {
        let mut a = app();
        a.handle_key(key('/'));
        for c in "zzz".chars() {
            a.handle_key(key(c));
        }
        let out = render(&a, 80, 10);
        assert!(out.contains("No recordings match the filter"));
        assert!(out.contains("zzz"), "filter text shown in the footer");
    }

    #[test]
    fn name_form_shows_typed_name_and_selected_mode() {
        let mut a = app();
        a.handle_key(key('r'));
        for c in "Sync".chars() {
            a.handle_key(key(c));
        }
        let out = render(&a, 90, 20);
        assert!(out.contains("New recording"));
        assert!(out.contains("Sync"));
        assert!(out.contains("◉ System + mic"));
        assert!(out.contains("○ Mic only"));
    }

    #[test]
    fn recording_screen_shows_timer_name_and_both_meters() {
        let mut a = app();
        a.on_recording_started("2026-10-22-1500-sync", CaptureMode::Mixed, None);
        a.on_recording_tick(
            Duration::from_secs(75),
            Some(LevelReading {
                sys: Some(0.3),
                mic: Some(0.0),
            }),
        );
        let out = render(&a, 90, 20);
        assert!(out.contains("REC"));
        assert!(out.contains("1:15"));
        assert!(out.contains("2026-10-22-1500-sync"));
        assert!(out.contains("system") && out.contains("mic"));
        assert!(out.contains('█'), "system meter should show some level");
    }

    #[test]
    fn mic_only_recording_marks_system_meter_off() {
        let mut a = app();
        a.on_recording_started("x", CaptureMode::MicOnly, None);
        let out = render(&a, 90, 20);
        assert!(out.contains("(off)"));
        assert!(out.contains("Mic only"));
    }

    #[test]
    fn low_disk_warning_is_visible_while_recording() {
        let mut a = app();
        a.on_recording_started("x", CaptureMode::Mixed, Some("only 200 MB free".into()));
        assert!(render(&a, 90, 20).contains("only 200 MB free"));
    }

    #[test]
    fn detail_shows_metadata_and_transcript_preview() {
        let mut a = app();
        a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        a.on_preview(Some("hello from the meeting".into()));
        let out = render(&a, 90, 16);
        assert!(out.contains("2026-10-22-1430-standup"));
        assert!(out.contains("2:05"));
        assert!(out.contains("yes"));
        assert!(out.contains("hello from the meeting"));
    }

    #[test]
    fn detail_without_transcript_says_none_yet() {
        let mut a = app();
        a.handle_key(key('j'));
        a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let out = render(&a, 90, 16);
        assert!(out.contains("none yet"));
        assert!(!out.contains("Transcript preview"));
    }

    #[test]
    fn help_lists_the_main_keys() {
        let mut a = app();
        a.handle_key(key('?'));
        let out = render(&a, 90, 24);
        for needle in ["record", "discard", "filter", "Recycle Bin / Trash", "quit"] {
            assert!(out.contains(needle), "help missing {needle:?}");
        }
    }

    #[test]
    fn delete_modal_names_the_recording_and_how_to_confirm() {
        let mut a = app();
        a.handle_key(key('d'));
        let out = render(&a, 100, 20);
        assert!(out.contains("Delete recording?"));
        assert!(out.contains("2026-10-22-1430-standup"));
        assert!(out.contains("y"));
    }

    #[test]
    fn status_message_replaces_the_hint_bar() {
        let mut a = app();
        a.on_recording_ended(Outcome::Saved, "2026-10-22-1500", "");
        let out = render(&a, 90, 12);
        assert!(out.contains("Saved 2026-10-22-1500"));
    }

    #[test]
    fn rendering_into_a_tiny_terminal_does_not_panic() {
        let mut a = app();
        for (w, h) in [(1, 1), (10, 3), (20, 5)] {
            render(&a, w, h);
        }
        a.handle_key(key('r'));
        render(&a, 12, 4);
        a.on_recording_started("x", CaptureMode::Mixed, Some("w".into()));
        render(&a, 12, 4);
        a.handle_key(key('x'));
        render(&a, 12, 4);
    }
}

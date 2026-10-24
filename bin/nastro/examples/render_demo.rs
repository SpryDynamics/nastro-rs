//! Renders `docs/assets/demo.svg`: an animated recording of the real `nastro` TUI, for the README.
//!
//! Nothing is mocked. It launches the actual binary in a pseudo-terminal (as the TUI tests do), points it at a few
//! made-up recordings in a temporary folder, sends scripted keystrokes, reads the screen back through a terminal
//! emulator after each one, and writes the result as an SVG whose frames are switched by CSS animation (which GitHub
//! displays inside README images, unlike scripts or video).
//!
//!     cargo build -p nastro
//!     cargo run -p nastro --example render_demo            # writes docs/assets/demo.svg
//!     cargo run -p nastro --example render_demo -- <nastro binary> <output.svg>
//!
//! (`eng/render-demo.ps1` does both steps.) The colors in the SVG are a fixed theme standing in for the viewer's
//! terminal theme, because the TUI itself only ever asks for the terminal's own 16 ANSI colors.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};

const COLS: u16 = 100;
const ROWS: u16 = 20;
const CELL_W: f64 = 8.4;
const CELL_H: f64 = 18.0;
const PAD: f64 = 22.0;

// ---------------------------------------------------------------------------------------------------- theme

/// A fixed stand-in for a terminal theme (Catppuccin Mocha). The TUI only requests ANSI colors by number, so any
/// theme would do; this one is readable and matches how the project is usually seen.
const FG: &str = "#cdd6f4";
const BG: &str = "#1e1e2e";
const ANSI: [&str; 16] = [
    "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
    "#6c7086", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
];

fn css_color(c: vt100::Color, default: &str) -> String {
    match c {
        vt100::Color::Default => default.to_string(),
        vt100::Color::Idx(i) => ANSI.get(usize::from(i)).copied().unwrap_or(FG).to_string(),
        vt100::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

// ----------------------------------------------------------------------------------------------- the screen

#[derive(Clone, PartialEq)]
struct Cell {
    text: String,
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    dim: bool,
    inverse: bool,
    /// The second half of a double-width character: draws nothing.
    continuation: bool,
}

#[derive(Clone, PartialEq)]
struct Frame {
    rows: Vec<Vec<Cell>>,
    hold_ms: u32,
}

fn snapshot(screen: &vt100::Screen) -> Vec<Vec<Cell>> {
    (0..ROWS)
        .map(|r| {
            (0..COLS)
                .map(|c| match screen.cell(r, c) {
                    Some(cell) => Cell {
                        text: if cell.has_contents() {
                            cell.contents().to_string()
                        } else {
                            " ".into()
                        },
                        fg: cell.fgcolor(),
                        bg: cell.bgcolor(),
                        bold: cell.bold(),
                        dim: cell.dim(),
                        inverse: cell.inverse(),
                        continuation: cell.is_wide_continuation(),
                    },
                    None => Cell {
                        text: " ".into(),
                        fg: vt100::Color::Default,
                        bg: vt100::Color::Default,
                        bold: false,
                        dim: false,
                        inverse: false,
                        continuation: false,
                    },
                })
                .collect()
        })
        .collect()
}

/// Replaces `from` with `to` wherever it appears in a row (the cells are one character each for ASCII), keeping the
/// style of the first replaced cell and padding the end of the row with blanks. The demo runs in a temporary folder
/// whose path contains the author's user name, and that must not end up in a public image.
fn redact(rows: &mut [Vec<Cell>], from: &str, to: &str) {
    let from_chars: Vec<char> = from.chars().collect();
    for row in rows {
        let line: Vec<char> = row
            .iter()
            .map(|c| c.text.chars().next().unwrap_or(' '))
            .collect();
        let Some(at) = line
            .windows(from_chars.len())
            .position(|w| w == from_chars.as_slice())
        else {
            continue;
        };
        let template = row[at].clone();
        let blank = Cell {
            text: " ".into(),
            bg: vt100::Color::Default,
            ..template.clone()
        };
        // The last cell of a row is usually a panel border, which must stay at the edge: replace in the rest of the
        // row and pad *before* it.
        let width = row.len();
        let edge = row[width - 1].clone();
        let mut out: Vec<Cell> = row[..at].to_vec();
        out.extend(to.chars().map(|ch| Cell {
            text: ch.to_string(),
            ..template.clone()
        }));
        out.extend_from_slice(&row[at + from_chars.len()..width - 1]);
        out.truncate(width - 1);
        out.resize(width - 1, blank);
        out.push(edge);
        *row = out;
    }
}

// ------------------------------------------------------------------------------------------------- the SVG

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The colors a cell is drawn with: (text, background).
fn resolve(c: &Cell) -> (String, Option<String>) {
    let (fg, bg) = (
        css_color(c.fg, FG),
        (c.bg != vt100::Color::Default).then(|| css_color(c.bg, BG)),
    );
    if c.inverse {
        (bg.unwrap_or_else(|| BG.to_string()), Some(fg))
    } else {
        (fg, bg)
    }
}

/// One frame's rows as SVG: background rectangles first, then a `<text>` per run of identically styled cells. Each
/// run's width is pinned with `textLength`, so columns stay aligned whichever monospace font the viewer has.
fn draw_frame(rows: &[Vec<Cell>], out: &mut String) {
    for (y, row) in rows.iter().enumerate() {
        let top = PAD + y as f64 * CELL_H;
        let mut x = 0;
        while x < row.len() {
            if row[x].continuation {
                x += 1;
                continue;
            }
            let key = (resolve(&row[x]), row[x].bold, row[x].dim);
            let mut end = x + 1;
            while end < row.len()
                && (row[end].continuation
                    || (resolve(&row[end]), row[end].bold, row[end].dim) == key)
            {
                end += 1;
            }
            let ((fg, bg), bold, dim) = key;
            let run = &row[x..end];
            let px = PAD + x as f64 * CELL_W;

            if let Some(bg) = bg {
                let _ = writeln!(
                    out,
                    r#"<rect x="{px:.1}" y="{top:.1}" width="{:.1}" height="{CELL_H}" fill="{bg}"/>"#,
                    (end - x) as f64 * CELL_W
                );
            }
            // Text only for the non-blank stretch of the run.
            let text: String = run
                .iter()
                .filter(|c| !c.continuation)
                .map(|c| c.text.as_str())
                .collect();
            let lead = text.chars().take_while(|c| *c == ' ').count();
            let body = text.trim();
            if !body.is_empty() {
                let w = body.chars().count() as f64 * CELL_W;
                let tx = px + lead as f64 * CELL_W;
                let weight = if bold { r#" font-weight="700""# } else { "" };
                let opacity = if dim { r#" opacity="0.6""# } else { "" };
                let _ = writeln!(
                    out,
                    r#"<text x="{tx:.1}" y="{:.1}" textLength="{w:.1}" lengthAdjust="spacing" fill="{fg}"{weight}{opacity}>{}</text>"#,
                    top + CELL_H - 5.0,
                    esc(body)
                );
            }
            x = end;
        }
    }
}

/// The whole animation. Frame `i` is visible only during its slice of the loop.
fn build_svg(frames: &[Frame]) -> String {
    let total_ms: u32 = frames.iter().map(|f| f.hold_ms).sum();
    let total_s = f64::from(total_ms) / 1000.0;
    let width = f64::from(COLS) * CELL_W + 2.0 * PAD;
    let height = f64::from(ROWS) * CELL_H + 2.0 * PAD;

    let mut css = String::new();
    let mut start_ms = 0u32;
    for (i, f) in frames.iter().enumerate() {
        let s = f64::from(start_ms) / f64::from(total_ms) * 100.0;
        let e = f64::from(start_ms + f.hold_ms) / f64::from(total_ms) * 100.0;
        let _ = write!(css, "@keyframes a{i}{{");
        if s > 0.0 {
            let _ = write!(css, "0%{{visibility:hidden}}");
        }
        let _ = write!(css, "{s:.3}%{{visibility:visible}}");
        if e < 100.0 {
            let _ = write!(css, "{e:.3}%{{visibility:hidden}}100%{{visibility:hidden}}");
        }
        css.push_str("}\n");
        let _ = writeln!(css, ".f{i}{{animation-name:a{i}}}");
        start_ms += f.hold_ms;
    }

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width:.0} {height:.0}" width="{width:.0}" height="{height:.0}" role="img" aria-label="An animated recording of the nastro terminal interface: browsing recordings, opening one to read its speaker-labelled transcript, the help screen, filtering, and the prompt before a speech model is downloaded.">"#
    );
    let _ = writeln!(svg, "<title>nastro terminal demo</title>");
    let _ = writeln!(svg, "<style>");
    let _ = writeln!(
        svg,
        "text{{font-family:ui-monospace,SFMono-Regular,'Cascadia Mono','JetBrains Mono',Menlo,Consolas,'DejaVu Sans Mono','Liberation Mono',monospace;font-size:14px;white-space:pre}}"
    );
    let _ = writeln!(
        svg,
        ".f{{visibility:hidden;animation-duration:{total_s:.2}s;animation-iteration-count:infinite;animation-timing-function:step-end}}"
    );
    let _ = writeln!(svg, ".f0{{visibility:visible}}"); // still readable where CSS animation is unsupported
    svg.push_str(&css);
    let _ = writeln!(svg, "</style>");
    let _ = writeln!(
        svg,
        r#"<rect width="{width:.0}" height="{height:.0}" rx="10" fill="{BG}"/>"#
    );
    for (i, f) in frames.iter().enumerate() {
        let _ = writeln!(svg, r#"<g class="f f{i}">"#);
        draw_frame(&f.rows, &mut svg);
        let _ = writeln!(svg, "</g>");
    }
    let _ = writeln!(svg, "</svg>");
    svg
}

// ------------------------------------------------------------------------------------- driving the real TUI

struct Term {
    screen: Arc<Mutex<vt100::Parser>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Term {
    fn launch(bin: &Path, config: &Path, model_dir: &Path) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open a pseudo-terminal");
        let mut cmd = CommandBuilder::new(bin);
        cmd.env("NASTRO_CONFIG", config);
        cmd.env("NASTRO_MODEL_DIR", model_dir);
        cmd.env("TERM", "xterm-256color");
        cmd.env_remove("NO_COLOR");
        let child = pair.slave.spawn_command(cmd).expect("start nastro");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("clone reader");
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer().expect("take writer")));
        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            std::num::NonZeroU16::new(ROWS).unwrap(),
            std::num::NonZeroU16::new(COLS).unwrap(),
            0,
        )));
        {
            let (screen, writer) = (Arc::clone(&screen), Arc::clone(&writer));
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    // Windows ConPTY asks where the cursor is and waits for an answer, as a real terminal gives.
                    if buf[..n].windows(4).any(|w| w == b"\x1b[6n") {
                        let mut w = writer.lock().unwrap();
                        let _ = w.write_all(b"\x1b[1;1R");
                        let _ = w.flush();
                    }
                    screen.lock().unwrap().process(&buf[..n]);
                }
            });
        }
        Term {
            screen,
            writer,
            child,
            _master: pair.master,
        }
    }

    fn send(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes).expect("write to the terminal");
        w.flush().unwrap();
    }

    fn text(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    fn cells(&self) -> Vec<Vec<Cell>> {
        snapshot(self.screen.lock().unwrap().screen())
    }

    fn wait_for(&self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.text().contains(needle) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}; screen:\n{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    fn wait_gone(&self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.text().contains(needle) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?} to go; screen:\n{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(40));
        }
    }
}

struct Recorder {
    term: Term,
    frames: Vec<Frame>,
    /// (text as it appears, text to show instead).
    redactions: Vec<(String, String)>,
}

impl Recorder {
    /// Captures the screen as it is now and holds it for `hold_ms` (merging with an identical previous frame).
    fn show(&mut self, hold_ms: u32) {
        std::thread::sleep(Duration::from_millis(220)); // let the UI's 100 ms tick redraw
        let mut rows = self.term.cells();
        for (from, to) in &self.redactions {
            redact(&mut rows, from, to);
        }
        match self.frames.last_mut() {
            Some(last) if last.rows == rows => last.hold_ms += hold_ms,
            _ => self.frames.push(Frame { rows, hold_ms }),
        }
    }

    fn press(&mut self, keys: &[u8], hold_ms: u32) {
        self.term.send(keys);
        self.show(hold_ms);
    }

    fn type_text(&mut self, text: &str, per_key_ms: u32) {
        for ch in text.chars() {
            let mut b = [0u8; 4];
            self.term.send(ch.encode_utf8(&mut b).as_bytes());
            self.show(per_key_ms);
        }
    }
}

// ----------------------------------------------------------------------------------------------- demo data

fn make_recording(root: &Path, name: &str, seconds: f64, size_mib: u64, transcript: Option<&str>) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let audio = std::fs::File::create(dir.join("audio.m4a")).unwrap();
    audio.set_len(size_mib * 1024 * 1024).unwrap();
    std::fs::write(
        dir.join("metadata.json"),
        format!(r#"{{"duration_seconds": {seconds}}}"#),
    )
    .unwrap();
    if let Some(t) = transcript {
        std::fs::write(dir.join("transcript.txt"), t).unwrap();
    }
}

const KICKOFF: &str = "[SPEAKER_00] Thanks everyone for joining. Let's start with the goals for the first phase.
[SPEAKER_01] We want the recorder on both platforms before the end of the quarter, with transcripts staying on the machine.
[SPEAKER_00] Agreed. Priya, can you own the audio capture side?
[SPEAKER_02] Yes. I'll start with the Windows loopback path and report back on Friday.
";

const REVIEW: &str = "[SPEAKER_00] The main question is where the speech model runs.
[SPEAKER_01] On the user's machine, always. No audio leaves the computer.
";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let default_bin = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.parent()?.parent().map(|d| {
                d.join(if cfg!(windows) {
                    "nastro.exe"
                } else {
                    "nastro"
                })
            })
        })
        .unwrap();
    let bin = args.get(1).map(PathBuf::from).unwrap_or(default_bin);
    let out = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs/assets/demo.svg"));
    assert!(
        bin.is_file(),
        "{} not found: run `cargo build -p nastro` first, or pass the binary's path",
        bin.display()
    );

    let tmp = tempfile::tempdir().unwrap();
    let recordings = tmp.path().join("recordings");
    make_recording(
        &recordings,
        "2026-08-13-1102-client-kickoff",
        2527.0,
        128,
        Some(KICKOFF),
    );
    make_recording(&recordings, "2026-08-13-0930-standup", 754.0, 38, None);
    make_recording(
        &recordings,
        "2026-08-11-1500-architecture-review",
        3311.0,
        168,
        Some(REVIEW),
    );
    let models = tmp.path().join("models"); // empty on purpose: pressing `t` then shows the download prompt
    std::fs::create_dir_all(&models).unwrap();
    let config = tmp.path().join("config.toml");
    // A TOML literal string (single quotes), so Windows backslashes need no escaping.
    std::fs::write(
        &config,
        format!("output_dir = '{}'\n", recordings.display()),
    )
    .unwrap();

    let term = Term::launch(&bin, &config, &models);
    term.wait_for("3 recordings");
    // The recordings folder, as nastro prints it (with the platform's separator), shown as a typical home path.
    let sep = std::path::MAIN_SEPARATOR;
    let shown = recordings.display().to_string();
    let mut r = Recorder {
        term,
        frames: Vec::new(),
        redactions: vec![
            (format!("{shown}{sep}"), "~/Recordings/nastro/".to_string()),
            (shown.clone(), "~/Recordings/nastro".to_string()),
        ],
    };

    // The list.
    r.show(2200);
    r.press(b"j", 800);
    r.press(b"j", 800);
    r.press(b"k", 500);
    r.press(b"k", 600);

    // Open a recording: its transcript, with who said what.
    r.term.send(b"\r");
    r.term.wait_for("Transcript preview");
    r.show(3600);
    r.term.send(b"\x1b");
    r.term.wait_gone("Transcript preview");
    r.show(600);

    // Help.
    r.press(b"?", 2400);
    r.term.send(b" ");
    r.term.wait_gone("Keys");
    r.show(500);

    // Filter as you type.
    r.press(b"/", 500);
    r.type_text("arch", 170);
    r.show(1400);
    r.term.send(b"\x1b");
    r.term.wait_for("standup");
    r.show(600);

    // `t` on a recording without a transcript: nastro asks before it downloads a model.
    r.press(b"j", 600);
    r.term.send(b"t");
    r.term.wait_for("Download speech model?");
    r.show(3200);
    r.term.send(b"n");
    r.term.wait_gone("Download speech model?");
    r.show(1600);

    r.term.send(b"q");
    let mut term = r.term;
    let deadline = Instant::now() + Duration::from_secs(10);
    while term.child.try_wait().expect("try_wait").is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = term.child.kill();

    let svg = build_svg(&r.frames);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(&out, &svg).unwrap();
    let total: u32 = r.frames.iter().map(|f| f.hold_ms).sum();
    println!(
        "wrote {} ({} frames, {:.1} s loop, {:.0} KB)",
        out.display(),
        r.frames.len(),
        f64::from(total) / 1000.0,
        svg.len() as f64 / 1024.0
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(s: &str) -> Vec<Cell> {
        s.chars()
            .map(|ch| Cell {
                text: ch.to_string(),
                fg: vt100::Color::Idx(6),
                bg: vt100::Color::Default,
                bold: false,
                dim: false,
                inverse: false,
                continuation: false,
            })
            .collect()
    }

    fn text(r: &[Cell]) -> String {
        r.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn redaction_replaces_the_path_keeps_the_rest_and_the_row_width() {
        let mut rows = vec![
            row(r"Folder  C:\Users\someone\Temp\x\recordings\2026-08-13 end   |"),
            row("nothing here"),
        ];
        let original_len = rows[0].len();
        redact(
            &mut rows,
            r"C:\Users\someone\Temp\x\recordings\",
            "~/Recordings/nastro/",
        );
        let shown = text(&rows[0]);
        assert!(
            shown.starts_with("Folder  ~/Recordings/nastro/2026-08-13 end"),
            "{shown}"
        );
        assert!(
            shown.ends_with('|'),
            "the border stays at the edge: {shown}"
        );
        assert_eq!(rows[0].len(), original_len, "the row keeps its width");
        assert_eq!(text(&rows[1]), "nothing here");
        assert!(!text(&rows[0]).contains("someone"));
    }

    #[test]
    fn redaction_keeps_the_panel_border_at_the_edge_of_the_row() {
        let mut rows = vec![row("| Folder C:\\long\\temp\\path\\x |")];
        let width = rows[0].len();
        redact(&mut rows, "C:\\long\\temp\\path\\", "~/R/");
        assert_eq!(rows[0].len(), width);
        assert_eq!(
            text(&rows[0]),
            format!("| Folder ~/R/x{}|", " ".repeat(width - 15))
        );
    }

    #[test]
    fn redaction_that_would_be_longer_never_overflows_the_row() {
        let mut rows = vec![row("a/b/c")];
        redact(&mut rows, "b", "a very long replacement");
        assert_eq!(rows[0].len(), 5);
    }

    #[test]
    fn the_svg_is_well_formed_has_every_frame_and_escapes_text() {
        let mut cells = row("a < b & c");
        cells.last_mut().unwrap().bold = true; // a bold last cell must not break the escaping of the rest
        let frames = vec![
            Frame {
                rows: vec![cells.clone()],
                hold_ms: 1000,
            },
            Frame {
                rows: vec![row("second")],
                hold_ms: 3000,
            },
        ];
        let svg = build_svg(&frames);
        assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
        assert_eq!(svg.matches(r#"<g class="f f"#).count(), 2);
        assert!(svg.contains("a &lt; b &amp;<"), "{svg}");
        assert!(
            !svg.contains("a < b"),
            "raw angle brackets would break the XML"
        );
        // 4 s loop: frame 0 is visible for the first 25%, frame 1 from 25% to the end.
        assert!(svg.contains("animation-duration:4.00s"));
        assert!(
            svg.contains("@keyframes a0{0.000%{visibility:visible}25.000%{visibility:hidden}"),
            "{svg}"
        );
        assert!(
            svg.contains("@keyframes a1{0%{visibility:hidden}25.000%{visibility:visible}}"),
            "{svg}"
        );
    }

    #[test]
    fn inverse_video_swaps_foreground_and_background() {
        let mut c = row("x").remove(0);
        c.inverse = true;
        let (fg, bg) = resolve(&c);
        assert_eq!(fg, BG);
        assert_eq!(bg.as_deref(), Some(ANSI[6]));
    }
}

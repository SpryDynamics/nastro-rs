//! Spawns and supervises the `nastro-tap` capture helper, speaking the stdio
//! protocol documented in [`crate::level`] / [`crate::exit`]: `level sys=.. mic=..`
//! lines on stdout, diagnostics on stderr, and the 0/2/1 exit-code contract.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::exit::{describe_tap_exit, TapExitDescription};
use crate::level::{parse_level_line, LevelReading};

const TAP_BINARY_NAME: &str = "nastro-tap";
const STDERR_TAIL_LINES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    Mixed,
    MicOnly,
    SystemOnly,
}

impl CaptureMode {
    fn tap_arg(self) -> Option<&'static str> {
        match self {
            CaptureMode::Mixed => None,
            CaptureMode::MicOnly => Some("--mic-only"),
            CaptureMode::SystemOnly => Some("--system-only"),
        }
    }
}

/// Find `nastro-tap` next to the currently running executable first (the normal
/// "shipped side by side" layout), then fall back to `PATH`.
pub fn resolve_tap_path() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        format!("{TAP_BINARY_NAME}.exe")
    } else {
        TAP_BINARY_NAME.to_string()
    };
    let exe_name = exe_name.as_str();

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            let candidate = dir.join(exe_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    which_on_path(exe_name)
}

fn which_on_path(exe_name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(exe_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[derive(Debug, thiserror::Error)]
pub enum TapError {
    #[error("nastro-tap not found next to the current executable or on PATH")]
    NotFound,
    #[error("failed to spawn {path}: {source}")]
    Spawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A running (or just-exited) `nastro-tap` child process, with its stdout level
/// readings streamed to a channel and its stderr tail captured for diagnostics.
pub struct TapProcess {
    child: Child,
    level_rx: Receiver<LevelReading>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl TapProcess {
    /// Spawn `nastro-tap` writing to `audio_path` in the given capture mode.
    pub fn spawn(tap_path: &Path, audio_path: &Path, mode: CaptureMode) -> Result<Self, TapError> {
        let mut command = Command::new(tap_path);
        command.arg(audio_path);
        if let Some(arg) = mode.tap_arg() {
            command.arg(arg);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(crate::stop::windows_creation_flags());
        }

        let mut child = command.spawn().map_err(|source| TapError::Spawn {
            path: tap_path.to_path_buf(),
            source,
        })?;

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (level_tx, level_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(reading) = parse_level_line(&line) {
                    // Receiver may already be gone if the caller dropped TapProcess
                    // without waiting; ignore send failures.
                    let _ = level_tx.send(reading);
                }
            }
        });

        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
        let stderr_tail_writer = Arc::clone(&stderr_tail);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut tail = stderr_tail_writer.lock().unwrap();
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });

        Ok(TapProcess {
            child,
            level_rx,
            stderr_tail,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Drain all level readings received so far without blocking.
    pub fn drain_levels(&self) -> Vec<LevelReading> {
        self.level_rx.try_iter().collect()
    }

    pub fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Ask the process to stop gracefully (SIGINT / CTRL_BREAK_EVENT).
    pub fn request_stop(&self) -> std::io::Result<()> {
        crate::stop::request_graceful_stop(self.pid())
    }

    /// Non-blocking check for whether the process has already exited on its own
    /// (e.g. a permission failure right after spawn, before any stop was
    /// requested). Returns `None` if it's still running.
    pub fn try_exit(&mut self) -> Option<TapExitDescription> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(describe_tap_exit(status.code(), &self.stderr_tail())),
            _ => None,
        }
    }

    /// Wait up to `timeout` for the process to exit on its own after
    /// [`request_stop`]; force-kill it if it hasn't by then. Always returns a
    /// description of how it ended.
    pub fn wait_with_timeout(&mut self, timeout: Duration) -> TapExitDescription {
        let deadline = std::time::Instant::now() + timeout;

        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return describe_tap_exit(status.code(), &self.stderr_tail()),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        return describe_tap_exit(None, &self.stderr_tail());
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return describe_tap_exit(None, &self.stderr_tail()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_command(script: &str) -> (&'static str, Vec<String>) {
        if cfg!(windows) {
            ("cmd", vec!["/C".to_string(), script.to_string()])
        } else {
            ("sh", vec!["-c".to_string(), script.to_string()])
        }
    }

    /// Smoke-test the stdio protocol plumbing (stdout level parsing, stderr tail
    /// capture, exit-code contract) against a trivial shell script standing in for
    /// the not-yet-implemented real `nastro-tap` binary.
    #[test]
    fn captures_level_lines_stderr_and_exit_code_from_a_fake_tap() {
        let (shell, base_args) =
            shell_command("echo level sys=0.30 mic=0.10 && echo Info: starting 1>&2 && exit 2");

        let mut command = Command::new(shell);
        command
            .args(&base_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().expect("spawn fake tap");
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let (level_tx, level_rx) = mpsc::channel();
        let stdout_thread = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(reading) = parse_level_line(&line) {
                    let _ = level_tx.send(reading);
                }
            }
        });

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let writer = Arc::clone(&stderr_tail);
        let stderr_thread = std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                writer.lock().unwrap().push_back(line);
            }
        });

        let status = child.wait().expect("wait for fake tap");
        stdout_thread.join().unwrap();
        stderr_thread.join().unwrap();

        let readings: Vec<_> = level_rx.try_iter().collect();
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].sys, Some(0.30));
        assert_eq!(readings[0].mic, Some(0.10));

        let tail = stderr_tail.lock().unwrap();
        assert!(tail.iter().any(|l| l.contains("starting")));

        let desc = describe_tap_exit(
            status.code(),
            &tail.iter().cloned().collect::<Vec<_>>().join("\n"),
        );
        assert_eq!(desc.kind, crate::exit::TapExitKind::PermissionDenied);
    }

    #[test]
    fn resolve_tap_path_does_not_panic_when_absent() {
        // We can't assert a specific result (depends on the host's PATH/build
        // output), just that resolution doesn't panic and returns an Option.
        let _ = resolve_tap_path();
    }
}

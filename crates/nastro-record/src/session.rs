//! A whole recording session as one object: lock, directory, anti-sleep guard and
//! the supervised `nastro-tap` child. Shared by the headless CLI and the TUI so
//! both get identical behaviour (private dirs, discard-if-too-short, cleanup of
//! failed starts, anti-sleep, graceful stop with a bounded force-kill).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{Datelike, Local, NaiveDateTime, Timelike};

use crate::discard::should_discard;
use crate::disk::check_low_disk_space;
use crate::exit::{TapExitDescription, TapExitKind};
use crate::level::LevelReading;
use crate::lock::{acquire_lock, LockError, LockHandle};
use crate::process::is_process_alive;
use crate::sleep_guard::{prevent_sleep, SleepGuard};
use crate::slug::slugify;
use crate::tap::{resolve_tap_path, CaptureMode, TapError, TapProcess};

const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const AUDIO_FILE_NAME: &str = "audio.m4a";

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("a recording is already in progress (pid {pid})")]
    AlreadyRecording { pid: u32 },
    #[error("nastro-tap not found next to the current executable or on PATH")]
    TapNotFound,
    #[error(transparent)]
    Lock(LockError),
    #[error("could not create recording directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Spawn(#[from] TapError),
}

/// What happened to the recording once the session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Audio kept on disk.
    Saved,
    /// Stopped cleanly but shorter than the keep threshold - treated as an
    /// accidental start/stop and removed.
    DiscardedTooShort,
    /// The user chose to discard it.
    DiscardedByUser,
    /// Capture failed and produced no audio; the empty directory was removed.
    FailedNoAudio,
}

#[derive(Debug, Clone)]
pub struct SessionEnd {
    pub record_dir: PathBuf,
    pub dir_name: String,
    pub exit: TapExitDescription,
    pub duration_seconds: f64,
    pub outcome: Outcome,
}

/// Pure outcome rule, separated out so it is unit-testable without any process.
///
/// A capture that crashed *after* producing audio keeps what it has: partial audio
/// is worth more than none, and the fragmented container is still playable.
pub fn decide_outcome(
    exit: TapExitKind,
    elapsed_seconds: f64,
    user_discard: bool,
    audio_bytes: u64,
) -> Outcome {
    if user_discard {
        return Outcome::DiscardedByUser;
    }
    match exit {
        TapExitKind::Clean => {
            if should_discard(elapsed_seconds) {
                Outcome::DiscardedTooShort
            } else {
                Outcome::Saved
            }
        }
        TapExitKind::PermissionDenied | TapExitKind::Failed => {
            if audio_bytes == 0 {
                Outcome::FailedNoAudio
            } else {
                Outcome::Saved
            }
        }
    }
}

/// `YYYY-MM-DD-HHMM[-slug]` for a start time and optional free-form name.
pub fn record_dir_name(now: NaiveDateTime, name: Option<&str>) -> String {
    let slug = name.map(slugify).filter(|s| !s.is_empty());
    nastro_records::build_dir_name(
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        slug.as_deref(),
    )
}

/// Pick a directory name that doesn't exist yet. Two recordings started in the
/// same minute with the same name would otherwise share a directory and the second
/// would overwrite the first's audio.
fn unique_dir_name(output_dir: &Path, base: &str) -> String {
    if !output_dir.join(base).exists() {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !output_dir.join(candidate).exists())
        .expect("unbounded range always yields a free name")
}

pub struct RecordingSession {
    record_dir: PathBuf,
    dir_name: String,
    mode: CaptureMode,
    warning: Option<String>,
    started: Instant,
    tap: TapProcess,
    done: bool,
    // Held for their Drop side effects (release lock / allow sleep again).
    _lock: LockHandle,
    _sleep: SleepGuard,
}

impl RecordingSession {
    /// Start recording using the `nastro-tap` next to this executable (or on PATH).
    pub fn start(
        output_dir: &Path,
        name: Option<&str>,
        mode: CaptureMode,
    ) -> Result<Self, SessionError> {
        let tap_path = resolve_tap_path().ok_or(SessionError::TapNotFound)?;
        Self::start_with(&tap_path, output_dir, name, mode)
    }

    pub fn start_with(
        tap_path: &Path,
        output_dir: &Path,
        name: Option<&str>,
        mode: CaptureMode,
    ) -> Result<Self, SessionError> {
        let lock = acquire_lock(output_dir, &is_process_alive).map_err(|e| match e {
            LockError::AlreadyRecording { pid } => SessionError::AlreadyRecording { pid },
            other => SessionError::Lock(other),
        })?;

        // After the lock call so the directory is guaranteed to exist for the query.
        let warning = check_low_disk_space(output_dir).ok().flatten();

        let base = record_dir_name(Local::now().naive_local(), name);
        let dir_name = unique_dir_name(output_dir, &base);
        let record_dir = output_dir.join(&dir_name);
        std::fs::create_dir_all(&record_dir).map_err(|source| SessionError::CreateDir {
            path: record_dir.clone(),
            source,
        })?;
        let _ = nastro_config::mode::ensure_private_dir(&record_dir);

        let audio_path = record_dir.join(AUDIO_FILE_NAME);
        let tap = match TapProcess::spawn(tap_path, &audio_path, mode) {
            Ok(tap) => tap,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&record_dir);
                return Err(e.into());
            }
        };

        Ok(Self {
            record_dir,
            dir_name,
            mode,
            warning,
            started: Instant::now(),
            tap,
            done: false,
            _lock: lock,
            _sleep: prevent_sleep(),
        })
    }

    pub fn record_dir(&self) -> &Path {
        &self.record_dir
    }

    pub fn dir_name(&self) -> &str {
        &self.dir_name
    }

    pub fn mode(&self) -> CaptureMode {
        self.mode
    }

    /// A low-disk-space warning computed at start, if any.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// The most recent level reading received since the last call, if any.
    pub fn latest_levels(&self) -> Option<LevelReading> {
        self.tap.drain_levels().into_iter().last()
    }

    /// If the capture process ended on its own (e.g. permission denied right after
    /// spawn), finish the session and return the result. `None` while still running.
    pub fn try_finish(&mut self) -> Option<SessionEnd> {
        let exit = self.tap.try_exit()?;
        Some(self.finish(exit, false))
    }

    /// Stop gracefully (force-killing after a bounded wait) and keep the audio
    /// unless it is too short.
    pub fn stop(mut self) -> SessionEnd {
        self.stop_inner(false)
    }

    /// Stop and delete the recording regardless of length.
    pub fn discard(mut self) -> SessionEnd {
        self.stop_inner(true)
    }

    fn stop_inner(&mut self, user_discard: bool) -> SessionEnd {
        let _ = self.tap.request_stop();
        let exit = self.tap.wait_with_timeout(STOP_TIMEOUT);
        self.finish(exit, user_discard)
    }

    fn finish(&mut self, exit: TapExitDescription, user_discard: bool) -> SessionEnd {
        self.done = true;
        let duration_seconds = self.started.elapsed().as_secs_f64();
        let audio_bytes = std::fs::metadata(self.record_dir.join(AUDIO_FILE_NAME))
            .map(|m| m.len())
            .unwrap_or(0);
        let outcome = decide_outcome(exit.kind, duration_seconds, user_discard, audio_bytes);

        if outcome != Outcome::Saved {
            let _ = std::fs::remove_dir_all(&self.record_dir);
        }

        SessionEnd {
            record_dir: self.record_dir.clone(),
            dir_name: self.dir_name.clone(),
            exit,
            duration_seconds,
            outcome,
        }
    }
}

impl Drop for RecordingSession {
    fn drop(&mut self) {
        // Never leave a capture process running (and the lock held by a dead UI)
        // if the session is dropped without an explicit stop, e.g. on a panic.
        if !self.done {
            let _ = self.tap.request_stop();
            let _ = self.tap.wait_with_timeout(STOP_TIMEOUT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 22)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[test]
    fn dir_name_without_a_name_is_just_the_timestamp() {
        assert_eq!(record_dir_name(at(14, 30), None), "2026-10-22-1430");
    }

    #[test]
    fn dir_name_slugifies_the_name() {
        assert_eq!(
            record_dir_name(at(9, 5), Some("Weekly Sync!")),
            "2026-10-22-0905-weekly-sync"
        );
    }

    #[test]
    fn dir_name_ignores_a_name_that_slugifies_to_nothing() {
        assert_eq!(record_dir_name(at(9, 5), Some("!!!")), "2026-10-22-0905");
    }

    #[test]
    fn dir_name_round_trips_through_the_records_parser() {
        let name = record_dir_name(at(14, 30), Some("Standup"));
        let parsed = nastro_records::parse_dir_name(&name).unwrap();
        assert_eq!((parsed.hour, parsed.minute), (14, 30));
        assert_eq!(parsed.slug.as_deref(), Some("standup"));
    }

    #[test]
    fn unique_dir_name_returns_base_when_free() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            unique_dir_name(dir.path(), "2026-10-22-1430"),
            "2026-10-22-1430"
        );
    }

    #[test]
    fn unique_dir_name_suffixes_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("2026-10-22-1430-standup")).unwrap();
        std::fs::create_dir(dir.path().join("2026-10-22-1430-standup-2")).unwrap();

        assert_eq!(
            unique_dir_name(dir.path(), "2026-10-22-1430-standup"),
            "2026-10-22-1430-standup-3"
        );
    }

    #[test]
    fn suffixed_dir_names_still_parse_as_recordings() {
        let parsed = nastro_records::parse_dir_name("2026-10-22-1430-standup-3").unwrap();
        assert_eq!(parsed.slug.as_deref(), Some("standup-3"));
        // And a suffix on a nameless recording is read as the slug, not rejected.
        assert!(nastro_records::parse_dir_name("2026-10-22-1430-2").is_some());
    }

    #[test]
    fn clean_long_recording_is_saved() {
        assert_eq!(
            decide_outcome(TapExitKind::Clean, 30.0, false, 5_000),
            Outcome::Saved
        );
    }

    #[test]
    fn clean_short_recording_is_discarded() {
        assert_eq!(
            decide_outcome(TapExitKind::Clean, 1.0, false, 5_000),
            Outcome::DiscardedTooShort
        );
    }

    #[test]
    fn user_discard_wins_over_everything() {
        for kind in [
            TapExitKind::Clean,
            TapExitKind::Failed,
            TapExitKind::PermissionDenied,
        ] {
            assert_eq!(
                decide_outcome(kind, 100.0, true, 9_999),
                Outcome::DiscardedByUser
            );
        }
    }

    #[test]
    fn failure_with_no_audio_cleans_up() {
        assert_eq!(
            decide_outcome(TapExitKind::Failed, 10.0, false, 0),
            Outcome::FailedNoAudio
        );
        assert_eq!(
            decide_outcome(TapExitKind::PermissionDenied, 0.1, false, 0),
            Outcome::FailedNoAudio
        );
    }

    #[test]
    fn failure_after_audio_was_written_keeps_the_partial_recording() {
        assert_eq!(
            decide_outcome(TapExitKind::Failed, 20.0, false, 40_000),
            Outcome::Saved
        );
    }

    #[test]
    fn start_with_a_missing_tap_binary_cleans_up_and_releases_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("recordings");
        let bogus = dir.path().join("no-such-tap");

        let err = RecordingSession::start_with(&bogus, &out, Some("x"), CaptureMode::Mixed)
            .err()
            .expect("spawning a missing binary must fail");
        assert!(matches!(err, SessionError::Spawn(_)), "got {err:?}");

        // The half-created recording dir was removed and the lock released, so a
        // retry isn't blocked by our own failed attempt.
        let leftovers: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }
}

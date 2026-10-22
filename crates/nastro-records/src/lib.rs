//! Filesystem-as-database record scanning for nastro.
//!
//! Ports the Go `internal/records` package: the recordings directory itself *is* the
//! database - one subdirectory per recording, named `YYYY-MM-DD-HHMM[-slug]`, each
//! holding `audio.m4a` / `metadata.json` / `transcript.txt` / `transcript.srt`. No
//! separate index file; `Scan` just lists and stats these directories.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct DirName {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub slug: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub dir_name: String,
    pub path: PathBuf,
    pub parsed: DirName,
    pub size_bytes: u64,
    pub duration_seconds: Option<f64>,
    pub has_transcript: bool,
}

#[derive(Debug, Deserialize, Default)]
struct Metadata {
    duration_seconds: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecordsError {
    #[error("failed to read directory {path}: {source}")]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Parse a recording directory name of the form `YYYY-MM-DD-HHMM` or
/// `YYYY-MM-DD-HHMM-slug`. Returns `None` for anything that doesn't match the
/// timestamp prefix - such entries are simply skipped during a scan rather than
/// treated as an error (a user might have stray files/dirs in the output root).
pub fn parse_dir_name(name: &str) -> Option<DirName> {
    // Expect exactly: 4 digits '-' 2 digits '-' 2 digits '-' 4 digits, optionally
    // followed by '-' and a free-form slug.
    let bytes = name.as_bytes();
    if bytes.len() < 15 {
        return None;
    }

    let is_digit_run =
        |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());

    let mut parts = name.splitn(5, '-');
    let year_s = parts.next()?;
    let month_s = parts.next()?;
    let day_s = parts.next()?;
    let hm_s = parts.next()?;
    let slug = parts.next().map(|s| s.to_string());

    if !is_digit_run(year_s, 4)
        || !is_digit_run(month_s, 2)
        || !is_digit_run(day_s, 2)
        || !is_digit_run(hm_s, 4)
    {
        return None;
    }

    let year: i32 = year_s.parse().ok()?;
    let month: u32 = month_s.parse().ok()?;
    let day: u32 = day_s.parse().ok()?;
    let hour: u32 = hm_s[0..2].parse().ok()?;
    let minute: u32 = hm_s[2..4].parse().ok()?;

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }

    Some(DirName {
        year,
        month,
        day,
        hour,
        minute,
        slug: slug.filter(|s| !s.is_empty()),
    })
}

/// Build a directory name from a timestamp and optional slug, the inverse of
/// [`parse_dir_name`]. Used by the record session orchestrator (`nastro-record`) when
/// starting a new recording.
pub fn build_dir_name(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    slug: Option<&str>,
) -> String {
    let base = format!("{year:04}-{month:02}-{day:02}-{hour:02}{minute:02}");
    match slug {
        Some(s) if !s.is_empty() => format!("{base}-{s}"),
        _ => base,
    }
}

fn parse_metadata(contents: &str) -> Option<f64> {
    serde_json::from_str::<Metadata>(contents)
        .ok()?
        .duration_seconds
}

/// Scan `root_dir` for recording directories. A missing root directory yields an
/// empty list, not an error - matching the Go implementation's "nothing recorded yet"
/// behavior on first run.
pub fn scan(root_dir: &Path) -> Result<Vec<Record>, RecordsError> {
    if !root_dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(root_dir).map_err(|source| RecordsError::ReadDir {
        path: root_dir.to_path_buf(),
        source,
    })?;

    let mut records = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        let Some(parsed) = parse_dir_name(dir_name) else {
            continue;
        };

        let size_bytes = fs::metadata(path.join("audio.m4a"))
            .map(|m| m.len())
            .unwrap_or(0);

        let duration_seconds = fs::read_to_string(path.join("metadata.json"))
            .ok()
            .and_then(|contents| parse_metadata(&contents));

        let has_transcript = path.join("transcript.txt").exists();

        records.push(Record {
            dir_name: dir_name.to_string(),
            path: path.clone(),
            parsed,
            size_bytes,
            duration_seconds,
            has_transcript,
        });
    }

    // Newest first.
    records.sort_by(|a, b| b.dir_name.cmp(&a.dir_name));

    Ok(records)
}

/// `12:34` for durations under an hour, `1:02:03` for an hour or more.
pub fn format_duration(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;

    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Human-readable byte size (`512 B`, `1.2 KB`, `3.4 MB`, `5.6 GB`), base-1024.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }

    let mut value = bytes as f64;
    let mut unit_idx = 0;
    while value >= 1024.0 && unit_idx < UNITS.len() - 1 {
        value /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit_idx])
    }
}

/// A tab-separated row for `nastro records --plain`, suitable for scripting.
pub fn plain_line(record: &Record) -> String {
    let duration = record
        .duration_seconds
        .map(format_duration)
        .unwrap_or_else(|| "-".to_string());
    let transcript_flag = if record.has_transcript { "yes" } else { "no" };

    format!(
        "{}\t{}\t{}\t{}",
        record.dir_name,
        duration,
        format_size(record.size_bytes),
        transcript_flag
    )
}

/// A short preview of a transcript file's contents (first `max_chars` characters,
/// trimmed), used by the TUI's detail view. Pure w.r.t. already-read content so it's
/// testable without touching disk.
pub fn transcript_preview(contents: &str, max_chars: usize) -> String {
    let trimmed = contents.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }

    let truncated: String = trimmed.chars().take(max_chars).collect();
    format!("{}…", truncated.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dir_name_valid_without_slug() {
        let parsed = parse_dir_name("2026-10-22-1430").unwrap();
        assert_eq!(
            parsed,
            DirName {
                year: 2026,
                month: 10,
                day: 22,
                hour: 14,
                minute: 30,
                slug: None
            }
        );
    }

    #[test]
    fn parse_dir_name_valid_with_slug() {
        let parsed = parse_dir_name("2026-10-22-1430-standup").unwrap();
        assert_eq!(parsed.slug.as_deref(), Some("standup"));
    }

    #[test]
    fn parse_dir_name_valid_with_multi_word_slug() {
        let parsed = parse_dir_name("2026-10-22-1430-weekly-sync-notes").unwrap();
        assert_eq!(parsed.slug.as_deref(), Some("weekly-sync-notes"));
    }

    #[test]
    fn parse_dir_name_rejects_garbage() {
        assert!(parse_dir_name("not-a-timestamp").is_none());
        assert!(parse_dir_name("").is_none());
        assert!(parse_dir_name(".git").is_none());
        assert!(parse_dir_name("2026-13-01-0000").is_none()); // invalid month
        assert!(parse_dir_name("2026-10-32-0000").is_none()); // invalid day
        assert!(parse_dir_name("2026-10-22-2460").is_none()); // invalid minute
        assert!(parse_dir_name("2026-10-22-2500").is_none()); // invalid hour
    }

    #[test]
    fn build_dir_name_round_trips_through_parse() {
        let built = build_dir_name(2026, 10, 22, 14, 30, Some("standup"));
        assert_eq!(built, "2026-10-22-1430-standup");

        let parsed = parse_dir_name(&built).unwrap();
        assert_eq!(parsed.slug.as_deref(), Some("standup"));
    }

    #[test]
    fn build_dir_name_without_slug() {
        assert_eq!(build_dir_name(2026, 1, 5, 9, 5, None), "2026-01-05-0905");
        assert_eq!(
            build_dir_name(2026, 1, 5, 9, 5, Some("")),
            "2026-01-05-0905"
        );
    }

    #[test]
    fn parse_metadata_reads_duration() {
        assert_eq!(parse_metadata(r#"{"duration_seconds": 42.5}"#), Some(42.5));
    }

    #[test]
    fn parse_metadata_missing_field_is_none() {
        assert_eq!(parse_metadata(r#"{}"#), None);
    }

    #[test]
    fn parse_metadata_malformed_json_is_none() {
        assert_eq!(parse_metadata("not json"), None);
    }

    #[test]
    fn format_duration_under_an_hour() {
        assert_eq!(format_duration(0.0), "0:00");
        assert_eq!(format_duration(5.0), "0:05");
        assert_eq!(format_duration(65.0), "1:05");
        assert_eq!(format_duration(599.0), "9:59");
    }

    #[test]
    fn format_duration_an_hour_or_more() {
        assert_eq!(format_duration(3600.0), "1:00:00");
        assert_eq!(format_duration(3661.0), "1:01:01");
        assert_eq!(format_duration(7325.0), "2:02:05");
    }

    #[test]
    fn format_size_bytes_and_up() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn transcript_preview_short_text_is_unchanged() {
        assert_eq!(transcript_preview("  hello world  ", 50), "hello world");
    }

    #[test]
    fn transcript_preview_truncates_long_text_with_ellipsis() {
        let long = "a".repeat(100);
        let preview = transcript_preview(&long, 10);
        assert_eq!(preview, format!("{}…", "a".repeat(10)));
    }

    #[test]
    fn scan_missing_root_dir_returns_empty_not_error() {
        let result = scan(Path::new("/this/does/not/exist/at/all")).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn scan_finds_valid_recording_dirs_and_skips_junk() {
        let root = tempfile::tempdir().unwrap();

        let rec1 = root.path().join("2026-10-20-0900-standup");
        fs::create_dir_all(&rec1).unwrap();
        fs::write(rec1.join("audio.m4a"), vec![0u8; 2048]).unwrap();
        fs::write(rec1.join("metadata.json"), r#"{"duration_seconds": 90.0}"#).unwrap();
        fs::write(rec1.join("transcript.txt"), "hello").unwrap();

        let rec2 = root.path().join("2026-10-21-1000");
        fs::create_dir_all(&rec2).unwrap();
        fs::write(rec2.join("audio.m4a"), vec![0u8; 1024]).unwrap();

        // Junk that should be skipped.
        fs::create_dir_all(root.path().join(".lock_leftover_dir")).unwrap();
        fs::write(root.path().join("stray-file.txt"), "ignore me").unwrap();

        let records = scan(root.path()).unwrap();

        assert_eq!(records.len(), 2);
        // Newest first.
        assert_eq!(records[0].dir_name, "2026-10-21-1000");
        assert_eq!(records[1].dir_name, "2026-10-20-0900-standup");

        let rec1_result = &records[1];
        assert_eq!(rec1_result.size_bytes, 2048);
        assert_eq!(rec1_result.duration_seconds, Some(90.0));
        assert!(rec1_result.has_transcript);

        let rec2_result = &records[0];
        assert_eq!(rec2_result.size_bytes, 1024);
        assert_eq!(rec2_result.duration_seconds, None);
        assert!(!rec2_result.has_transcript);
    }

    #[test]
    fn plain_line_formats_tab_separated_row() {
        let record = Record {
            dir_name: "2026-10-22-1430-standup".to_string(),
            path: PathBuf::from("/tmp/2026-10-22-1430-standup"),
            parsed: parse_dir_name("2026-10-22-1430-standup").unwrap(),
            size_bytes: 1536,
            duration_seconds: Some(65.0),
            has_transcript: true,
        };

        assert_eq!(
            plain_line(&record),
            "2026-10-22-1430-standup\t1:05\t1.5 KB\tyes"
        );
    }

    #[test]
    fn plain_line_handles_missing_duration_and_transcript() {
        let record = Record {
            dir_name: "2026-10-22-1430".to_string(),
            path: PathBuf::from("/tmp/2026-10-22-1430"),
            parsed: parse_dir_name("2026-10-22-1430").unwrap(),
            size_bytes: 0,
            duration_seconds: None,
            has_transcript: false,
        };

        assert_eq!(plain_line(&record), "2026-10-22-1430\t-\t0 B\tno");
    }
}

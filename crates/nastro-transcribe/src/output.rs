//! Transcript formatting (`transcript.txt`, `transcript.srt`) and crash-safe writing.

use std::fs;
use std::path::Path;

use crate::{Error, Result};

/// File names inside a recording directory. `transcript.txt` is what the rest of nastro looks for.
pub const TXT_FILE: &str = "transcript.txt";
pub const SRT_FILE: &str = "transcript.srt";

/// One recognised phrase with its position in the recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// Collapses all whitespace (including newlines) to single spaces. A newline inside a cue would end it early
/// in SRT, and `transcript.txt` is one segment per line.
fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `HH:MM:SS,mmm` as SRT requires. Negative input is clamped to zero.
pub fn srt_time(ms: i64) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

/// Plain text: one segment per line.
pub fn to_txt(segments: &[Segment]) -> String {
    let mut out = String::new();
    for s in segments {
        let text = clean(&s.text);
        if !text.is_empty() {
            out.push_str(&text);
            out.push('\n');
        }
    }
    out
}

/// SubRip: numbered cues with `start --> end` timestamps.
pub fn to_srt(segments: &[Segment]) -> String {
    let mut out = String::new();
    let mut n = 0;
    for s in segments {
        let text = clean(&s.text);
        if text.is_empty() {
            continue;
        }
        n += 1;
        // A cue must have positive length for most players.
        let end = s.end_ms.max(s.start_ms + 1);
        out.push_str(&format!(
            "{n}\n{} --> {}\n{text}\n\n",
            srt_time(s.start_ms),
            srt_time(end)
        ));
    }
    out
}

/// Writes `transcript.txt` and `transcript.srt` into `dir` so that a failure at any point leaves any previous
/// transcript exactly as it was: both new files are written and permission-tightened under temporary names
/// first, and only then moved into place (`.txt` last, because that is the file whose existence means
/// "transcribed").
pub fn write_all(dir: &Path, segments: &[Segment]) -> Result<()> {
    let files = [(SRT_FILE, to_srt(segments)), (TXT_FILE, to_txt(segments))];
    let mut staged = Vec::new();

    let cleanup = |staged: &[(std::path::PathBuf, std::path::PathBuf)]| {
        for (tmp, _) in staged {
            let _ = fs::remove_file(tmp);
        }
    };

    for (name, content) in &files {
        let tmp = dir.join(format!("{name}.partial"));
        let dest = dir.join(name);
        let result = fs::write(&tmp, content.as_bytes())
            .map_err(|e| Error::io(format!("writing {}", tmp.display()), e))
            .and_then(|_| {
                // Private from the first moment it is complete: transcripts hold whatever was said.
                nastro_config::mode::ensure_private_file(&tmp)
                    .map_err(|e| Error::io(format!("securing {}", tmp.display()), e))
            });
        staged.push((tmp, dest));
        if let Err(e) = result {
            cleanup(&staged);
            return Err(e);
        }
    }

    for (i, (tmp, dest)) in staged.iter().enumerate() {
        if let Err(e) = fs::rename(tmp, dest) {
            cleanup(&staged[i..]);
            return Err(Error::io(format!("saving {}", dest.display()), e));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(a: i64, b: i64, t: &str) -> Segment {
        Segment {
            start_ms: a,
            end_ms: b,
            text: t.into(),
        }
    }

    #[test]
    fn srt_timestamps() {
        assert_eq!(srt_time(0), "00:00:00,000");
        assert_eq!(srt_time(1_234), "00:00:01,234");
        assert_eq!(srt_time(61_005), "00:01:01,005");
        assert_eq!(srt_time(3_661_999), "01:01:01,999");
        assert_eq!(srt_time(100 * 3_600_000), "100:00:00,000");
        assert_eq!(srt_time(-50), "00:00:00,000");
    }

    #[test]
    fn txt_is_one_trimmed_line_per_segment_and_skips_empties() {
        let out = to_txt(&[
            seg(0, 1, "  Hello   there "),
            seg(1, 2, "   "),
            seg(2, 3, "line\nbreak"),
        ]);
        assert_eq!(out, "Hello there\nline break\n");
        assert_eq!(to_txt(&[]), "");
    }

    #[test]
    fn srt_numbers_only_real_cues_and_never_contains_a_blank_line_inside_one() {
        let out = to_srt(&[
            seg(0, 1500, "First"),
            seg(1500, 1500, ""),
            seg(2000, 3000, "a\n\nb"),
        ]);
        assert_eq!(
            out,
            "1\n00:00:00,000 --> 00:00:01,500\nFirst\n\n2\n00:00:02,000 --> 00:00:03,000\na b\n\n"
        );
    }

    #[test]
    fn zero_length_cues_get_a_minimum_duration() {
        assert!(to_srt(&[seg(5000, 5000, "x")]).contains("00:00:05,000 --> 00:00:05,001"));
    }

    #[test]
    fn write_all_creates_both_files_and_no_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        write_all(dir.path(), &[seg(0, 1000, "hi")]).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(TXT_FILE)).unwrap(),
            "hi\n"
        );
        assert!(fs::read_to_string(dir.path().join(SRT_FILE))
            .unwrap()
            .contains("-->"));
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn write_all_replaces_an_existing_transcript() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(TXT_FILE), "old\n").unwrap();
        write_all(dir.path(), &[seg(0, 1000, "new")]).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(TXT_FILE)).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn a_failed_write_leaves_the_previous_transcript_untouched() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(TXT_FILE), "keep me\n").unwrap();
        // Block the staging name for the .srt with a directory, so staging fails after nothing was promoted.
        fs::create_dir(dir.path().join(format!("{SRT_FILE}.partial"))).unwrap();

        let err = write_all(dir.path(), &[seg(0, 1000, "new")]);

        assert!(err.is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join(TXT_FILE)).unwrap(),
            "keep me\n"
        );
        assert!(!dir.path().join(format!("{TXT_FILE}.partial")).exists());
    }

    #[cfg(unix)]
    #[test]
    fn transcripts_are_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write_all(dir.path(), &[seg(0, 1000, "secret")]).unwrap();
        for f in [TXT_FILE, SRT_FILE] {
            let mode = fs::metadata(dir.path().join(f))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{f}");
        }
    }
}

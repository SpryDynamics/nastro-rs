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
    /// Who said it (`SPEAKER_00`, ...), when speaker identification ran. `None` for plain transcription.
    pub speaker: Option<String>,
}

/// The label for a phrase that speaker identification could not attribute to anyone.
pub const UNKNOWN_SPEAKER: &str = "SPEAKER_UNKNOWN";

/// Consecutive phrases by the same speaker, merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub speaker: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// `true` if any phrase carries a speaker, i.e. the transcript should be written with speaker labels.
pub fn has_speakers(segments: &[Segment]) -> bool {
    segments.iter().any(|s| s.speaker.is_some())
}

/// Merges consecutive same-speaker phrases (empty ones dropped) into runs, from the first phrase's start to the
/// last phrase's end. A phrase with no speaker is attributed to [`UNKNOWN_SPEAKER`].
pub fn runs(segments: &[Segment]) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for s in segments {
        let text = clean(&s.text);
        if text.is_empty() {
            continue;
        }
        let speaker = s.speaker.as_deref().unwrap_or(UNKNOWN_SPEAKER);
        match out.last_mut() {
            Some(last) if last.speaker == speaker => {
                last.end_ms = s.end_ms;
                last.text.push(' ');
                last.text.push_str(&text);
            }
            _ => out.push(Run {
                speaker: speaker.to_string(),
                start_ms: s.start_ms,
                end_ms: s.end_ms,
                text,
            }),
        }
    }
    out
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

/// Plain text: one segment per line, or, when speakers are known, one `[SPEAKER_00] ...` line per run.
pub fn to_txt(segments: &[Segment]) -> String {
    if has_speakers(segments) {
        return runs(segments)
            .iter()
            .map(|r| format!("[{}] {}\n", r.speaker, r.text))
            .collect();
    }
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

/// SubRip: numbered cues with `start --> end` timestamps (one per speaker run, prefixed with `[SPEAKER_00]`,
/// when speakers are known).
pub fn to_srt(segments: &[Segment]) -> String {
    if has_speakers(segments) {
        let mut out = String::new();
        for (i, r) in runs(segments).iter().enumerate() {
            let end = r.end_ms.max(r.start_ms + 1);
            out.push_str(&format!(
                "{}\n{} --> {}\n[{}] {}\n\n",
                i + 1,
                srt_time(r.start_ms),
                srt_time(end),
                r.speaker,
                r.text
            ));
        }
        return out;
    }
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
            speaker: None,
        }
    }

    fn said(who: Option<&str>, a: i64, b: i64, t: &str) -> Segment {
        Segment {
            speaker: who.map(String::from),
            ..seg(a, b, t)
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

    // ----- speaker labels

    #[test]
    fn plain_transcripts_are_unchanged_when_no_speaker_is_known() {
        let segs = [seg(0, 1000, "Hello"), seg(1000, 2000, "there")];
        assert!(!has_speakers(&segs));
        assert_eq!(to_txt(&segs), "Hello\nthere\n");
        assert!(!to_srt(&segs).contains('['));
    }

    #[test]
    fn consecutive_segments_by_one_speaker_merge_into_one_labelled_line() {
        let segs = [
            said(Some("SPEAKER_00"), 0, 1000, "Good morning."),
            said(Some("SPEAKER_00"), 1200, 3000, "Let's begin."),
            said(Some("SPEAKER_01"), 3500, 4000, "Thanks."),
            said(Some("SPEAKER_00"), 4500, 5000, "Sure."),
        ];
        assert!(has_speakers(&segs));
        assert_eq!(
            to_txt(&segs),
            "[SPEAKER_00] Good morning. Let's begin.\n[SPEAKER_01] Thanks.\n[SPEAKER_00] Sure.\n"
        );
        let r = runs(&segs);
        assert_eq!(r.len(), 3);
        assert_eq!(
            (r[0].start_ms, r[0].end_ms),
            (0, 3000),
            "a run spans first start to last end"
        );
    }

    #[test]
    fn srt_has_one_numbered_cue_per_run_with_the_speaker_prefix() {
        let segs = [
            said(Some("SPEAKER_00"), 0, 1000, "One."),
            said(Some("SPEAKER_00"), 1000, 2000, "Two."),
            said(Some("SPEAKER_01"), 2500, 3000, "Three."),
        ];
        assert_eq!(
            to_srt(&segs),
            "1\n00:00:00,000 --> 00:00:02,000\n[SPEAKER_00] One. Two.\n\n2\n00:00:02,500 --> 00:00:03,000\n[SPEAKER_01] Three.\n\n"
        );
    }

    #[test]
    fn an_unattributed_phrase_is_labelled_unknown_once_any_speaker_is_known() {
        let segs = [
            said(Some("SPEAKER_00"), 0, 1000, "Known."),
            said(None, 1000, 2000, "Mystery."),
        ];
        assert_eq!(
            to_txt(&segs),
            "[SPEAKER_00] Known.\n[SPEAKER_UNKNOWN] Mystery.\n"
        );
    }

    #[test]
    fn empty_phrases_do_not_split_or_create_runs() {
        let segs = [
            said(Some("SPEAKER_00"), 0, 1000, "A."),
            said(Some("SPEAKER_01"), 1000, 1500, "   "),
            said(Some("SPEAKER_00"), 1500, 2000, "B."),
        ];
        assert_eq!(to_txt(&segs), "[SPEAKER_00] A. B.\n");
        assert_eq!(to_srt(&segs).matches("-->").count(), 1);
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

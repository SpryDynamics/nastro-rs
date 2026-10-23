//! Platform actions the TUI delegates to the OS: moving a recording to the
//! Recycle Bin / freedesktop Trash, and revealing it in the file manager.

use std::path::Path;
use std::process::{Command, Stdio};

/// Move a recording directory to the Recycle Bin / Trash.
///
/// Deliberately does **not** fall back to a permanent delete if the trash is
/// unavailable (the original project's macOS build did). A recoverable delete that
/// silently turns into an irreversible one is the wrong failure mode for someone's
/// meeting recordings - the user gets an error and can delete by hand if they mean it.
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| format!("Couldn't move to the Recycle Bin / Trash: {e}"))
}

/// Open the file manager on a recording. On Windows the audio file is
/// pre-selected in Explorer; elsewhere the folder is opened with `xdg-open`.
pub fn reveal(record_dir: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let audio = record_dir.join("audio.m4a");
        let target = if audio.exists() {
            audio
        } else {
            record_dir.to_path_buf()
        };
        // `raw_arg` so a path containing spaces is quoted for Explorer exactly once.
        // Explorer exits with status 1 even when it succeeds, so only a failure to
        // launch is treated as an error.
        Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", target.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Couldn't open Explorer: {e}"))
    }
    #[cfg(not(windows))]
    {
        Command::new("xdg-open")
            .arg(record_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Couldn't run xdg-open: {e}"))
    }
}

/// Read a recording's transcript for the detail preview, if there is one.
pub fn read_transcript(record_dir: &Path) -> Option<String> {
    std::fs::read_to_string(record_dir.join("transcript.txt")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_transcript_returns_contents_when_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("transcript.txt"), "hello world").unwrap();
        assert_eq!(read_transcript(dir.path()).as_deref(), Some("hello world"));
    }

    #[test]
    fn read_transcript_is_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_transcript(dir.path()), None);
    }

    /// Really moves a directory to this machine's Recycle Bin / Trash, so it is not
    /// run by default (and a CI agent may have no trash at all). Run locally with
    ///   cargo test -p nastro-tui -- --ignored move_to_trash
    #[test]
    #[ignore]
    fn move_to_trash_removes_the_directory_from_its_original_location() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("2026-01-01-0000-nastro-trash-test");
        std::fs::create_dir(&victim).unwrap();
        std::fs::write(victim.join("audio.m4a"), b"x").unwrap();

        move_to_trash(&victim).expect("trash should be available on a desktop machine");
        assert!(
            !victim.exists(),
            "directory should be gone from its original location"
        );
    }

    #[test]
    fn move_to_trash_of_a_missing_path_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let err = move_to_trash(&dir.path().join("does-not-exist")).unwrap_err();
        assert!(err.contains("Recycle Bin / Trash"), "{err}");
    }
}

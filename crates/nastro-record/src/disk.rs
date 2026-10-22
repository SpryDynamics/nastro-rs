//! Low-disk-space warning, ported from the Go `internal/record` package's
//! `checkLowDiskSpace`/`diskSpaceWarning`.

use std::path::Path;

/// Below this many free bytes on the output filesystem, warn the user before/while
/// recording - matches the original project's 1 GiB threshold.
pub const LOW_DISK_THRESHOLD_BYTES: u64 = 1024 * 1024 * 1024;

/// Pure formatting logic, decoupled from the actual filesystem query so it's
/// trivially unit-testable.
pub fn disk_space_warning(free_bytes: u64) -> Option<String> {
    if free_bytes >= LOW_DISK_THRESHOLD_BYTES {
        return None;
    }

    Some(format!(
        "Warning: only {} free - recording may fail to save if space runs out.",
        nastro_records::format_size(free_bytes)
    ))
}

/// Query real available space on the filesystem containing `path` and run it
/// through [`disk_space_warning`]. `path` should exist (or have an existing parent)
/// for the platform disk-space query to succeed.
pub fn check_low_disk_space(path: &Path) -> std::io::Result<Option<String>> {
    let free = fs4::available_space(path)?;
    Ok(disk_space_warning(free))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_below_threshold() {
        let warning = disk_space_warning(500 * 1024 * 1024);
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("500.0 MB"));
    }

    #[test]
    fn no_warning_at_or_above_threshold() {
        assert!(disk_space_warning(LOW_DISK_THRESHOLD_BYTES).is_none());
        assert!(disk_space_warning(LOW_DISK_THRESHOLD_BYTES * 2).is_none());
    }

    #[test]
    fn check_low_disk_space_against_real_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        // Just confirming it doesn't error on a real, existing path - we can't
        // assert a specific value since available space varies by machine/CI agent.
        let result = check_low_disk_space(dir.path());
        assert!(result.is_ok());
    }
}

//! Single global recording lock, ported from the Go `internal/record` package's
//! `AcquireLock`/`ReleaseLock`: one `.lock` file at the *root* of the output
//! directory (not per-recording - only one recording session system-wide at a
//! time), containing the holder's PID, with atomic create (`O_CREATE|O_EXCL`
//! equivalent) and stale-lock detection/recovery.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("a recording is already in progress (pid {pid})")]
    AlreadyRecording { pid: u32 },
    #[error("I/O error acquiring lock at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Held for the lifetime of a recording session; dropping it releases the lock
/// (removes the lockfile).
pub struct LockHandle {
    path: PathBuf,
}

impl LockHandle {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Explicit release, if you want to handle the `io::Result` instead of
    /// swallowing it in `Drop`.
    pub fn release(self) -> std::io::Result<()> {
        let path = self.path.clone();
        std::mem::forget(self); // skip Drop's best-effort removal, we're doing it here
        std::fs::remove_file(path)
    }
}

impl Drop for LockHandle {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn lock_path(output_dir: &Path) -> PathBuf {
    output_dir.join(".lock")
}

/// Acquire the global recording lock under `output_dir`, creating the directory
/// (with private permissions, best-effort on Windows) if needed.
///
/// `is_process_alive` is injected so the stale-lock-recovery logic is unit
/// testable without needing to spawn/kill real processes; see
/// [`crate::process::is_process_alive`] for the real implementation.
pub fn acquire_lock(
    output_dir: &Path,
    is_process_alive: &dyn Fn(u32) -> bool,
) -> Result<LockHandle, LockError> {
    std::fs::create_dir_all(output_dir).map_err(|source| LockError::Io {
        path: output_dir.to_path_buf(),
        source,
    })?;
    let _ = nastro_config::mode::ensure_private_dir(output_dir);

    let path = lock_path(output_dir);

    loop {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let pid = std::process::id();
                file.write_all(pid.to_string().as_bytes())
                    .map_err(|source| LockError::Io {
                        path: path.clone(),
                        source,
                    })?;
                drop(file);
                let _ = nastro_config::mode::ensure_private_file(&path);
                return Ok(LockHandle { path });
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let holder_pid = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|content| content.trim().parse::<u32>().ok());

                match holder_pid {
                    Some(pid) if is_process_alive(pid) => {
                        return Err(LockError::AlreadyRecording { pid });
                    }
                    _ => {
                        // Stale (holder process is gone) or corrupt content - clean up
                        // and retry the atomic create.
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                }
            }
            Err(source) => return Err(LockError::Io { path, source }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn always_alive(_pid: u32) -> bool {
        true
    }

    fn never_alive(_pid: u32) -> bool {
        false
    }

    #[test]
    fn acquires_lock_in_fresh_dir_and_writes_own_pid() {
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("recordings");

        let handle = acquire_lock(&output_dir, &always_alive).unwrap();

        let content = std::fs::read_to_string(handle.path()).unwrap();
        assert_eq!(content.trim().parse::<u32>().unwrap(), std::process::id());
    }

    #[test]
    fn release_removes_the_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        let handle = acquire_lock(dir.path(), &always_alive).unwrap();
        let path = handle.path().to_path_buf();

        assert!(path.exists());
        handle.release().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn drop_releases_the_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        let path;
        {
            let handle = acquire_lock(dir.path(), &always_alive).unwrap();
            path = handle.path().to_path_buf();
            assert!(path.exists());
        }
        assert!(!path.exists());
    }

    #[test]
    fn concurrent_conflict_when_holder_is_alive() {
        let dir = tempfile::tempdir().unwrap();
        let _first = acquire_lock(dir.path(), &always_alive).unwrap();

        let result = acquire_lock(dir.path(), &always_alive);

        assert!(matches!(result, Err(LockError::AlreadyRecording { .. })));
    }

    #[test]
    fn stale_lock_is_recovered_when_holder_is_dead() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(lock_path(dir.path()), "999999").unwrap();

        // `never_alive` simulates the recorded PID no longer existing.
        let handle = acquire_lock(dir.path(), &never_alive).unwrap();

        let content = std::fs::read_to_string(handle.path()).unwrap();
        assert_eq!(content.trim().parse::<u32>().unwrap(), std::process::id());
    }

    #[test]
    fn corrupt_lock_content_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(lock_path(dir.path()), "not-a-pid").unwrap();

        // Corrupt content is treated as stale regardless of is_process_alive.
        let handle = acquire_lock(dir.path(), &always_alive).unwrap();

        assert!(handle.path().exists());
    }

    #[test]
    fn empty_lock_file_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(lock_path(dir.path()), "").unwrap();

        let handle = acquire_lock(dir.path(), &always_alive).unwrap();
        assert!(handle.path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn lockfile_is_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let handle = acquire_lock(dir.path(), &always_alive).unwrap();

        let mode = std::fs::metadata(handle.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, nastro_config::PRIVATE_FILE_MODE);
    }

    #[cfg(unix)]
    #[test]
    fn output_dir_is_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("recordings");
        let _handle = acquire_lock(&output_dir, &always_alive).unwrap();

        let mode = std::fs::metadata(&output_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, nastro_config::PRIVATE_DIR_MODE);
    }
}

//! Anti-sleep guard, held for the duration of a recording. Replaces the original
//! macOS implementation's `caffeinate -dims -w <tap-pid>` subprocess.
//!
//! Windows: `SetThreadExecutionState` - reliable, no external dependency.
//! Linux: best-effort `systemd-inhibit --what=sleep` child process; if
//! `systemd-inhibit` isn't on PATH (non-systemd distros), recording proceeds anyway
//! with a one-time warning rather than failing.

/// RAII guard: sleep is prevented for as long as this is alive, and the previous
/// power-management state is restored on drop.
pub struct SleepGuard {
    #[cfg(unix)]
    inhibitor_child: Option<std::process::Child>,
}

#[cfg(windows)]
pub fn prevent_sleep() -> SleepGuard {
    use windows::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
    };

    // SAFETY: plain FFI call, no pointers/lifetimes involved.
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED);
    }

    SleepGuard {}
}

#[cfg(windows)]
impl Drop for SleepGuard {
    fn drop(&mut self) {
        use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};
        // SAFETY: plain FFI call. Passing just ES_CONTINUOUS clears any previous
        // requirement set by this thread.
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

#[cfg(unix)]
pub fn prevent_sleep() -> SleepGuard {
    use std::process::{Command, Stdio};

    let child = Command::new("systemd-inhibit")
        .args([
            "--what=sleep",
            "--who=nastro",
            "--why=recording in progress",
            "--mode=block",
            "sleep",
            "infinity",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    match child {
        Ok(child) => SleepGuard {
            inhibitor_child: Some(child),
        },
        Err(_) => {
            eprintln!(
                "warning: 'systemd-inhibit' not available - the system may sleep during recording."
            );
            SleepGuard {
                inhibitor_child: None,
            }
        }
    }
}

#[cfg(unix)]
impl Drop for SleepGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.inhibitor_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prevent_sleep_and_drop_do_not_panic() {
        // Platform-specific behavior can't be meaningfully asserted in CI (no way to
        // inspect OS power state / systemd-inhibit presence portably), but this at
        // least exercises both the acquire and release paths without crashing.
        let guard = prevent_sleep();
        drop(guard);
    }
}

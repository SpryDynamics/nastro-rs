//! Graceful-stop signal, platform-specific.
//!
//! Unix: `SIGINT`, same as the original macOS implementation.
//! Windows: there is no SIGINT equivalent for an arbitrary child process; the
//! documented approach is `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, ...)`, which
//! only reaches processes in the *same console process group* as the target - so
//! the child must be spawned with `CREATE_NEW_PROCESS_GROUP` (see
//! [`windows_creation_flags`]) for this to work without also signaling ourselves.

use std::io;

/// Windows process-creation flags to pass to `Command::creation_flags` when
/// spawning `nastro-tap`, so that a later [`request_graceful_stop`] can target it
/// (and only it) via `GenerateConsoleCtrlEvent`.
#[cfg(windows)]
pub fn windows_creation_flags() -> u32 {
    use windows::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
    CREATE_NEW_PROCESS_GROUP.0
}

#[cfg(windows)]
pub fn request_graceful_stop(pid: u32) -> io::Result<()> {
    use windows::Win32::System::Console::{GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT};

    // SAFETY: GenerateConsoleCtrlEvent is a plain FFI call; `pid` identifies the
    // target process group (the child must have been created with
    // CREATE_NEW_PROCESS_GROUP so this doesn't also signal our own process).
    unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) }
        .map_err(|_| io::Error::last_os_error())
}

#[cfg(unix)]
pub fn request_graceful_stop(pid: u32) -> io::Result<()> {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    kill(Pid::from_raw(pid as i32), Signal::SIGINT)
        .map_err(|errno| io::Error::from_raw_os_error(errno as i32))
}

#[cfg(unix)]
pub fn windows_creation_flags() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requesting_stop_on_a_nonexistent_pid_is_an_error() {
        // An implausibly large PID that (almost certainly) doesn't correspond to a
        // running process on either platform.
        let result = request_graceful_stop(u32::MAX - 1);
        assert!(result.is_err());
    }
}

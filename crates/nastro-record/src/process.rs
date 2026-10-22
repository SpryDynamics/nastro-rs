//! Cross-platform "is this PID still alive?" check, backing stale-lock detection.
//! Replaces the Go implementation's POSIX-only `syscall.Kill(pid, 0)` with the
//! `sysinfo` crate, which works uniformly on Windows and Linux.

use sysinfo::{Pid, ProcessesToUpdate, System};

pub fn is_process_alive(pid: u32) -> bool {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    system.process(Pid::from_u32(pid)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn implausible_pid_is_not_alive() {
        // PID 0 is a reserved special-case on both platforms (the Linux scheduler /
        // Windows "System Idle Process"), so it isn't a reliable "doesn't exist"
        // probe - use an implausibly large PID instead, far beyond either
        // platform's real PID range.
        assert!(!is_process_alive(u32::MAX - 1));
    }
}

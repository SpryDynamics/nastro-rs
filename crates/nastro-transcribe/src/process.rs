//! Starting and stopping external helper processes (ffmpeg, whisperx, uv) reliably.

use std::process::{Child, Command};

/// Prepares `cmd` so that [`kill_tree`] can later stop it *and everything it starts*.
///
/// That matters because WhisperX is Python: on Windows the `whisperx.exe` that `uv`/`pipx` install is only a
/// launcher that starts the real interpreter as a child, and killing just the launcher would leave Python
/// running (and burning every CPU core) in the background.
pub(crate) fn configure(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // its own group, so the whole tree can be signalled at once
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Kills `child` and all of its descendants. Best effort, and the child is reaped afterwards.
pub(crate) fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        use nix::sys::signal::{killpg, Signal};
        use nix::unistd::Pid;
        if let Ok(pid) = i32::try_from(child.id()) {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // taskkill /T walks the process tree; /F because Python does not honour a polite request mid-inference.
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill(); // in case the above was not available
    let _ = child.wait();
}

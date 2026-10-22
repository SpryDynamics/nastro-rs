//! `nastro-tap` exit-code contract, ported from the Go `internal/record` package's
//! `DescribeTapExit`.
//!
//! Exit codes:
//!   0 - clean stop (graceful signal received, flushed and exited)
//!   2 - permission denied (microphone / system-audio capture access refused)
//!   1 (or anything else, or terminated by signal) - other failure

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapExitKind {
    Clean,
    PermissionDenied,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapExitDescription {
    pub kind: TapExitKind,
    pub message: String,
}

/// `code` is `None` when the process was terminated by a signal (Unix) rather than
/// exiting normally. `stderr_tail` is the last captured diagnostic output from
/// `nastro-tap`'s stderr, if any, appended to the message for context.
pub fn describe_tap_exit(code: Option<i32>, stderr_tail: &str) -> TapExitDescription {
    let suffix = |base: String| -> String {
        if stderr_tail.trim().is_empty() {
            base
        } else {
            format!("{base} ({})", stderr_tail.trim())
        }
    };

    match code {
        Some(0) => TapExitDescription {
            kind: TapExitKind::Clean,
            message: "Recording stopped.".to_string(),
        },
        Some(2) => TapExitDescription {
            kind: TapExitKind::PermissionDenied,
            message: suffix(
                "Permission denied: grant microphone / system-audio capture access and try again."
                    .to_string(),
            ),
        },
        Some(code) => TapExitDescription {
            kind: TapExitKind::Failed,
            message: suffix(format!("nastro-tap exited with code {code}.")),
        },
        None => TapExitDescription {
            kind: TapExitKind::Failed,
            message: suffix("nastro-tap was terminated unexpectedly.".to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_zero_is_clean() {
        let desc = describe_tap_exit(Some(0), "");
        assert_eq!(desc.kind, TapExitKind::Clean);
        assert_eq!(desc.message, "Recording stopped.");
    }

    #[test]
    fn exit_code_two_is_permission_denied() {
        let desc = describe_tap_exit(Some(2), "");
        assert_eq!(desc.kind, TapExitKind::PermissionDenied);
        assert!(desc.message.contains("Permission denied"));
    }

    #[test]
    fn exit_code_two_includes_stderr_tail() {
        let desc = describe_tap_exit(Some(2), "WASAPI: access denied by OS\n");
        assert!(desc.message.contains("WASAPI: access denied by OS"));
    }

    #[test]
    fn other_exit_codes_are_failures() {
        let desc = describe_tap_exit(Some(1), "");
        assert_eq!(desc.kind, TapExitKind::Failed);
        assert!(desc.message.contains("code 1"));
    }

    #[test]
    fn signal_termination_is_a_failure() {
        let desc = describe_tap_exit(None, "");
        assert_eq!(desc.kind, TapExitKind::Failed);
        assert!(desc.message.contains("terminated"));
    }

    #[test]
    fn empty_stderr_tail_produces_no_trailing_parens() {
        let desc = describe_tap_exit(Some(1), "   \n");
        assert!(!desc.message.contains('('));
    }
}

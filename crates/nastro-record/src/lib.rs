//! Recording session orchestration for nastro.
//!
//! Ports the Go `internal/record` package: slugify/dir-naming helpers, the global
//! recording lockfile lifecycle, disk-space warnings, the discard-if-too-short rule,
//! anti-sleep, graceful-stop signal handling, and the stdio protocol with the
//! `nastro-tap` capture helper (`level sys=.. mic=..` lines + the 0/2/1 exit-code
//! contract).

mod discard;
mod disk;
mod exit;
mod level;
mod lock;
mod process;
mod sleep_guard;
mod slug;
mod stop;
mod tap;

pub use discard::{should_discard, MIN_KEEPABLE_SECONDS};
pub use disk::{check_low_disk_space, disk_space_warning, LOW_DISK_THRESHOLD_BYTES};
pub use exit::{describe_tap_exit, TapExitDescription, TapExitKind};
pub use level::{parse_level_line, LevelReading};
pub use lock::{acquire_lock, lock_path, LockError, LockHandle};
pub use process::is_process_alive;
pub use sleep_guard::{prevent_sleep, SleepGuard};
pub use slug::slugify;
pub use stop::request_graceful_stop;
pub use tap::{resolve_tap_path, CaptureMode, TapError, TapProcess};

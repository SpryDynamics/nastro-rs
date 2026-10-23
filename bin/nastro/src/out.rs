//! Standard output that behaves like a normal command-line tool when the reader goes away.
//!
//! Rust ignores SIGPIPE, so a plain `println!` panics ("failed printing to stdout: Broken pipe", exit status 101)
//! when output is piped into something that exits early, such as `nastro records --plain | head`. Here a closed
//! pipe is simply the end of the conversation: stop quietly with success, as `ls | head` does.

use std::fmt::Arguments;
use std::io::{ErrorKind, Write};

/// Writes one line to stdout. Ends the program with status 0 if the pipe is closed, and with an error message
/// and status 1 for any other write failure (such as a full disk).
pub fn line(args: Arguments<'_>) {
    let mut stdout = std::io::stdout().lock();
    if let Err(e) = writeln!(stdout, "{args}") {
        if e.kind() == ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        eprintln!("error: could not write to standard output: {e}");
        std::process::exit(1);
    }
}

/// `println!`, but safe against a closed pipe (see the module docs).
macro_rules! outln {
    () => {
        $crate::out::line(format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::out::line(format_args!($($arg)*))
    };
}

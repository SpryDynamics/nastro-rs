//! Audio capture helper (placeholder - fleshed out in a later phase).
//!
//! Will implement WASAPI loopback capture (Windows) / PulseAudio monitor-source
//! capture (Linux) + mic capture, mixing, and incremental AAC/M4A muxing via an
//! ffmpeg subprocess, reporting `level sys=.. mic=..` lines on stdout and using the
//! exit-code contract (0 = clean stop, 2 = permission denied, 1 = other failure)
//! documented in the main nastro crate.

fn main() {
    eprintln!("nastro-tap: capture not yet implemented in the Rust port.");
    std::process::exit(1);
}

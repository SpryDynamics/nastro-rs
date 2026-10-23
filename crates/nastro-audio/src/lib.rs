//! Audio capture/mixing core for nastro.
//!
//! Platform-agnostic pieces (fully unit tested, no audio hardware needed):
//! [`AudioRingBuffer`], [`SampleMixer`], [`RateWatcher`], [`compute_level`].
//!
//! Platform capture backends (require real audio hardware/services, validated
//! manually rather than in CI): WASAPI loopback + mic capture on Windows,
//! PulseAudio/PipeWire monitor-source + mic capture on Linux. Both expose the
//! same [`capture::start_system_capture`]/[`capture::start_mic_capture`]
//! signatures so callers don't need their own `#[cfg(...)]`.

mod capture;
mod level;
mod mixer;
mod rate_watcher;
mod ring_buffer;

#[cfg(windows)]
mod windows;

#[cfg(all(unix, target_os = "linux"))]
mod linux;

pub use capture::{
    start_mic_capture, start_system_capture, CaptureError, CaptureHandle, TARGET_SAMPLE_RATE,
};
pub use level::compute_level;
pub use mixer::SampleMixer;
pub use rate_watcher::RateWatcher;
pub use ring_buffer::AudioRingBuffer;

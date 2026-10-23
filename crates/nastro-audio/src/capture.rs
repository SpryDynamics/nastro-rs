//! Platform-agnostic capture backend surface. Each platform module
//! ([`crate::windows`] / [`crate::linux`]) implements
//! [`start_system_capture`]/[`start_mic_capture`] with the same signatures, so
//! `nastro-tap` doesn't need any `#[cfg(...)]` of its own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::AudioRingBuffer;

/// Internal target format for both capture sources - chosen to match WASAPI's
/// overwhelmingly common shared-mode mix rate, minimizing real resampling work.
pub const TARGET_SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("no capture device is available for {0}")]
    NoDevice(&'static str),
    #[error("failed to initialize {component} capture: {message}")]
    Init {
        component: &'static str,
        message: String,
    },
}

/// A running capture thread. Dropping it (or calling [`CaptureHandle::stop`])
/// signals the capture loop to exit and joins the thread.
pub struct CaptureHandle {
    stop_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl CaptureHandle {
    pub(crate) fn new(stop_flag: Arc<AtomicBool>, join_handle: JoinHandle<()>) -> Self {
        Self {
            stop_flag,
            join_handle: Some(join_handle),
        }
    }

    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Start capturing *system audio* (everything currently playing, loopback-style)
/// into `sink` at [`TARGET_SAMPLE_RATE`], mono. Returns once the capture thread
/// has either confirmed successful device initialization or failed to start.
#[cfg(windows)]
pub fn start_system_capture(sink: Arc<AudioRingBuffer>) -> Result<CaptureHandle, CaptureError> {
    crate::windows::start_system_capture(sink)
}

#[cfg(windows)]
pub fn start_mic_capture(sink: Arc<AudioRingBuffer>) -> Result<CaptureHandle, CaptureError> {
    crate::windows::start_mic_capture(sink)
}

#[cfg(all(unix, target_os = "linux"))]
pub fn start_system_capture(sink: Arc<AudioRingBuffer>) -> Result<CaptureHandle, CaptureError> {
    crate::linux::start_system_capture(sink)
}

#[cfg(all(unix, target_os = "linux"))]
pub fn start_mic_capture(sink: Arc<AudioRingBuffer>) -> Result<CaptureHandle, CaptureError> {
    crate::linux::start_mic_capture(sink)
}

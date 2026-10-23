//! Linux capture backend.
//!
//! System audio: PulseAudio's "monitor source" of the default sink, via the
//! blocking `libpulse-simple-binding` API - this also works transparently under
//! PipeWire, since PipeWire implements the PulseAudio protocol
//! (`pipewire-pulse`). The default sink's monitor name is resolved by shelling
//! out to `pactl get-default-sink` rather than pulling in the full async
//! Context/Introspector API, keeping this backend's surface small.
//!
//! Microphone: the same blocking `Simple` API, opened against the default
//! source (no device override needed).

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction as PulseDirection;
use libpulse_simple_binding::Simple;

use crate::capture::{CaptureError, CaptureHandle, TARGET_SAMPLE_RATE};
use crate::AudioRingBuffer;

const READ_CHUNK_BYTES: usize = 4096;

pub fn start_system_capture(sink: Arc<AudioRingBuffer>) -> Result<CaptureHandle, CaptureError> {
    let monitor_device = default_sink_monitor_name()?;
    spawn_pulse_capture("system", Some(monitor_device), sink)
}

pub fn start_mic_capture(sink: Arc<AudioRingBuffer>) -> Result<CaptureHandle, CaptureError> {
    spawn_pulse_capture("mic", None, sink)
}

fn default_sink_monitor_name() -> Result<String, CaptureError> {
    let output = Command::new("pactl")
        .arg("get-default-sink")
        .stdin(Stdio::null())
        .output()
        .map_err(|_| CaptureError::NoDevice("system"))?;

    if !output.status.success() {
        return Err(CaptureError::NoDevice("system"));
    }

    let sink_name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sink_name.is_empty() {
        return Err(CaptureError::NoDevice("system"));
    }

    Ok(format!("{sink_name}.monitor"))
}

fn spawn_pulse_capture(
    label: &'static str,
    device: Option<String>,
    sink: Arc<AudioRingBuffer>,
) -> Result<CaptureHandle, CaptureError> {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let thread_stop_flag = Arc::clone(&stop_flag);
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), CaptureError>>();

    let join_handle = thread::Builder::new()
        .name(format!("nastro-tap-{label}-capture"))
        .spawn(move || {
            run_pulse_capture_thread(label, device.as_deref(), sink, thread_stop_flag, ready_tx)
        })
        .map_err(|e| CaptureError::Init {
            component: label,
            message: e.to_string(),
        })?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(CaptureHandle::new(stop_flag, join_handle)),
        Ok(Err(err)) => {
            let _ = join_handle.join();
            Err(err)
        }
        Err(_) => {
            let _ = join_handle.join();
            Err(CaptureError::Init {
                component: label,
                message: "capture thread exited before reporting readiness".to_string(),
            })
        }
    }
}

fn run_pulse_capture_thread(
    label: &'static str,
    device: Option<&str>,
    sink: Arc<AudioRingBuffer>,
    stop_flag: Arc<AtomicBool>,
    ready_tx: mpsc::Sender<Result<(), CaptureError>>,
) {
    let spec = Spec {
        format: Format::FLOAT32NE,
        channels: 1,
        rate: TARGET_SAMPLE_RATE,
    };
    let description = if device.is_some() {
        "system audio capture"
    } else {
        "microphone capture"
    };

    let simple = match Simple::new(
        None, // default server
        "nastro",
        PulseDirection::Record,
        device,
        description,
        &spec,
        None,
        None,
    ) {
        Ok(s) => s,
        Err(e) => {
            // `PAErr` has an inherent `to_string(&self) -> Option<String>` (wrapping
            // `pa_strerror`, which can return NULL for an unrecognized code) that
            // shadows the blanket `Display`/`ToString` impl - use `{:?}` to sidestep
            // it rather than unwrap an `Option` that's allowed to be `None`.
            let _ = ready_tx.send(Err(CaptureError::Init {
                component: label,
                message: format!("{e:?}"),
            }));
            return;
        }
    };

    let _ = ready_tx.send(Ok(()));

    let mut buf = [0u8; READ_CHUNK_BYTES];
    while !stop_flag.load(Ordering::SeqCst) {
        match simple.read(&mut buf) {
            Ok(()) => {
                // `READ_CHUNK_BYTES` is a multiple of 4, so there is no remainder.
                let (words, _) = buf.as_chunks::<4>();
                let samples: Vec<f32> = words.iter().map(|w| f32::from_le_bytes(*w)).collect();
                sink.push(&samples);
            }
            Err(e) => {
                log::warn!("{label} capture read error: {e}");
                break;
            }
        }
    }
}

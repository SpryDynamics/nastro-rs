//! Windows capture backend: WASAPI loopback (system audio) + normal capture
//! (microphone), both in shared mode with `autoconvert` so the audio engine
//! handles resampling/downmixing to our target mono format - no manual format
//! negotiation needed.
//!
//! System-wide loopback is set up per the `wasapi` crate's documented pattern:
//! get the *default render* (speaker/output) device, then initialize its
//! `AudioClient` for `Direction::Capture` instead of `Direction::Render` - this is
//! the WASAPI idiom for "capture everything currently playing", equivalent to the
//! original macOS implementation's `CATapDescription(stereoGlobalTapButExcludeProcesses: [])`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use wasapi::{initialize_mta, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

use crate::capture::{CaptureError, CaptureHandle, TARGET_SAMPLE_RATE};
use crate::AudioRingBuffer;

/// How long to wait for a new buffer before checking the stop flag again. Short
/// enough for a responsive Ctrl+C -> stop, long enough not to busy-loop.
const EVENT_WAIT_TIMEOUT_MS: u32 = 100;

pub fn start_system_capture(sink: Arc<AudioRingBuffer>) -> Result<CaptureHandle, CaptureError> {
    spawn_capture("system", Direction::Render, sink)
}

pub fn start_mic_capture(sink: Arc<AudioRingBuffer>) -> Result<CaptureHandle, CaptureError> {
    spawn_capture("mic", Direction::Capture, sink)
}

/// `device_direction` is the *endpoint's* direction (Render = speakers, for
/// loopback; Capture = microphone, for normal capture). The `AudioClient` is
/// always initialized for `Direction::Capture` regardless - that's what makes the
/// Render-endpoint case a loopback capture rather than a (nonsensical) playback.
fn spawn_capture(
    label: &'static str,
    device_direction: Direction,
    sink: Arc<AudioRingBuffer>,
) -> Result<CaptureHandle, CaptureError> {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let thread_stop_flag = Arc::clone(&stop_flag);
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), CaptureError>>();

    let join_handle = thread::Builder::new()
        .name(format!("nastro-tap-{label}-capture"))
        .spawn(move || {
            run_capture_thread(label, device_direction, sink, thread_stop_flag, ready_tx);
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
            // The thread panicked/dropped the sender before reporting readiness.
            let _ = join_handle.join();
            Err(CaptureError::Init {
                component: label,
                message: "capture thread exited before reporting readiness".to_string(),
            })
        }
    }
}

fn run_capture_thread(
    label: &'static str,
    device_direction: Direction,
    sink: Arc<AudioRingBuffer>,
    stop_flag: Arc<AtomicBool>,
    ready_tx: mpsc::Sender<Result<(), CaptureError>>,
) {
    if let Err(e) = initialize_mta().ok() {
        let _ = ready_tx.send(Err(CaptureError::Init {
            component: label,
            message: format!("COM initialization failed: {e:?}"),
        }));
        return;
    }

    let result = (|| -> Result<_, CaptureError> {
        let enumerator = DeviceEnumerator::new().map_err(|e| CaptureError::Init {
            component: label,
            message: e.to_string(),
        })?;
        let device = enumerator
            .get_default_device(&device_direction)
            .map_err(|_| CaptureError::NoDevice(label))?;
        let mut audio_client = device.get_iaudioclient().map_err(|e| CaptureError::Init {
            component: label,
            message: e.to_string(),
        })?;

        let desired_format = WaveFormat::new(
            32,
            32,
            &SampleType::Float,
            TARGET_SAMPLE_RATE as usize,
            1,
            None,
        );

        let (_default_period, min_period) =
            audio_client
                .get_device_period()
                .map_err(|e| CaptureError::Init {
                    component: label,
                    message: e.to_string(),
                })?;

        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: min_period,
        };

        audio_client
            .initialize_client(&desired_format, &Direction::Capture, &mode)
            .map_err(|e| CaptureError::Init {
                component: label,
                message: e.to_string(),
            })?;

        let event_handle = audio_client
            .set_get_eventhandle()
            .map_err(|e| CaptureError::Init {
                component: label,
                message: e.to_string(),
            })?;
        let capture_client =
            audio_client
                .get_audiocaptureclient()
                .map_err(|e| CaptureError::Init {
                    component: label,
                    message: e.to_string(),
                })?;

        Ok((audio_client, event_handle, capture_client))
    })();

    let (audio_client, event_handle, capture_client) = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    if let Err(e) = audio_client.start_stream() {
        let _ = ready_tx.send(Err(CaptureError::Init {
            component: label,
            message: e.to_string(),
        }));
        return;
    }

    let _ = ready_tx.send(Ok(()));

    let mut byte_queue: std::collections::VecDeque<u8> = std::collections::VecDeque::new();

    while !stop_flag.load(Ordering::SeqCst) {
        match capture_client.read_from_device_to_deque(&mut byte_queue) {
            Ok(_buffer_info) => {}
            Err(e) => {
                log::warn!("{label} capture read error: {e}");
                break;
            }
        }

        // 4 bytes per f32 sample, mono.
        let whole_samples = byte_queue.len() / 4;
        if whole_samples > 0 {
            let mut samples = Vec::with_capacity(whole_samples);
            for _ in 0..whole_samples {
                let bytes = [
                    byte_queue.pop_front().unwrap(),
                    byte_queue.pop_front().unwrap(),
                    byte_queue.pop_front().unwrap(),
                    byte_queue.pop_front().unwrap(),
                ];
                samples.push(f32::from_le_bytes(bytes));
            }
            sink.push(&samples);
        }

        if event_handle.wait_for_event(EVENT_WAIT_TIMEOUT_MS).is_err() {
            // Timeout or wait error - just loop back and re-check the stop flag
            // rather than treating it as fatal, so a quiet source (e.g. no system
            // audio currently playing) doesn't look like a crash.
            continue;
        }
    }

    let _ = audio_client.stop_stream();
    wasapi::deinitialize();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Manual smoke test against real hardware - not run in CI (no guarantee a
    /// build agent has a working audio device), run locally with:
    ///   cargo test -p nastro-audio -- --ignored windows_live_smoke_test
    #[test]
    #[ignore]
    fn windows_live_smoke_test_system_capture_receives_real_samples() {
        let sink = Arc::new(AudioRingBuffer::new(TARGET_SAMPLE_RATE as usize * 5));
        let handle = start_system_capture(Arc::clone(&sink)).expect("system capture should start");

        std::thread::sleep(Duration::from_millis(800));
        let received = sink.len();
        handle.stop();

        println!("captured {received} system-audio samples in ~800ms");
        assert!(received > 0, "expected some samples to have been captured");
    }

    #[test]
    #[ignore]
    fn windows_live_smoke_test_mic_capture_receives_real_samples() {
        let sink = Arc::new(AudioRingBuffer::new(TARGET_SAMPLE_RATE as usize * 5));
        let handle = start_mic_capture(Arc::clone(&sink)).expect("mic capture should start");

        std::thread::sleep(Duration::from_millis(800));
        let received = sink.len();
        handle.stop();

        println!("captured {received} mic samples in ~800ms");
        assert!(received > 0, "expected some samples to have been captured");
    }
}

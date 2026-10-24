//! Speech recognition with whisper.cpp (statically linked through `whisper-rs`).

use std::ffi::c_void;
use std::os::raw::c_int;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::audio::SAMPLE_RATE;
use crate::output::Segment;
use crate::{CancelToken, Error, Result};

/// What recognition produced.
#[derive(Debug, Clone)]
pub struct Recognition {
    pub segments: Vec<Segment>,
    /// The language whisper used (the detected one when the request was `auto`).
    pub language: String,
}

/// whisper.cpp is built for a portable x86-64 baseline (see `.cargo/config.toml`) that requires these. Without
/// them it would die with an illegal-instruction fault, so say so up front instead.
pub fn cpu_supported() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
            && std::arch::is_x86_feature_detected!("f16c")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

/// `true` for the English-only `*.en` models.
pub fn is_english_only(model: &str) -> bool {
    let stem = Path::new(model)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| model.to_string());
    stem.contains(".en.") || stem.contains(".en-") || stem.ends_with(".en")
}

/// Validates the requested language against the model and returns what to hand to whisper
/// (`"auto"` or a language code).
pub fn resolve_language(model: &str, requested: &str) -> Result<String> {
    // Validating a language calls into whisper.cpp, which logs unknown ones straight to stderr.
    install_log_hooks();
    let lang = requested.trim().to_ascii_lowercase();
    let lang = if lang.is_empty() {
        "auto".to_string()
    } else {
        lang
    };

    if is_english_only(model) {
        return match lang.as_str() {
            "auto" | "en" => Ok("en".to_string()),
            _ => Err(Error::EnglishOnlyModel {
                model: model.to_string(),
                lang,
            }),
        };
    }
    if lang == "auto" || whisper_rs::get_lang_id(&lang).is_some() {
        Ok(lang)
    } else {
        Err(Error::UnknownLanguage(requested.to_string()))
    }
}

pub(crate) fn install_log_hooks() {
    static ONCE: Once = Once::new();
    // whisper.cpp prints model/system info to stderr by default, which would scribble over the TUI. Route it
    // through `log`, where it is dropped (nastro installs no logger).
    ONCE.call_once(whisper_rs::install_logging_hooks);
}

/// State shared with whisper.cpp's C callbacks (through their `user_data` pointer) and the polling thread.
struct CallbackState {
    cancel: Arc<AtomicBool>,
    progress: AtomicI32,
}

// These are used instead of whisper-rs's `set_progress_callback_safe` / `set_abort_callback_safe`.
// whisper-rs 0.16's `set_abort_callback_safe` registers a trampoline for the closure's concrete type but hands
// it a pointer to a boxed trait object, so any closure that captures something is called through the wrong
// type (undefined behaviour; observed as a spurious "abort" from whisper_full, error code -6).
// Here `user_data` is a `*const CallbackState` and nothing else, and is only ever read through shared
// references to atomics, so the callbacks are sound as long as the pointee outlives `state.full(..)`, which
// `recognize` guarantees by holding the `Arc` across the call.
unsafe extern "C" fn abort_trampoline(user_data: *mut c_void) -> bool {
    // SAFETY: `user_data` is `Arc::as_ptr` of a live `CallbackState` (see `recognize`).
    let state = &*(user_data as *const CallbackState);
    state.cancel.load(Ordering::Relaxed)
}

unsafe extern "C" fn progress_trampoline(
    _ctx: *mut whisper_rs::whisper_rs_sys::whisper_context,
    _state: *mut whisper_rs::whisper_rs_sys::whisper_state,
    progress: c_int,
    user_data: *mut c_void,
) {
    // SAFETY: as for `abort_trampoline`.
    let state = &*(user_data as *const CallbackState);
    state.progress.store(progress, Ordering::Relaxed);
}
/// Transcribes 16 kHz mono `samples` with the model at `model_path`.
///
/// `on_progress` receives 0..=100 while recognition runs. Cancelling aborts inside whisper (within a fraction
/// of a second) and returns [`Error::Cancelled`].
pub fn recognize(
    model_path: &Path,
    language: &str,
    threads: usize,
    samples: &[f32],
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(u8),
) -> Result<Recognition> {
    if !cpu_supported() {
        return Err(Error::UnsupportedCpu);
    }
    cancel.check()?;
    install_log_hooks();

    let path = model_path.to_str().ok_or_else(|| {
        Error::Whisper(format!(
            "the model path {} is not valid UTF-8",
            model_path.display()
        ))
    })?;

    let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default()).map_err(
        |e| {
            Error::Whisper(format!(
                "could not load the model {}: {e}",
                model_path.display()
            ))
        },
    )?;
    let mut state = ctx
        .create_state()
        .map_err(|e| Error::Whisper(format!("could not start recognition: {e}")))?;

    // whisper refuses clips under one second; pad very short ones with silence instead of failing.
    let min_len = SAMPLE_RATE as usize + SAMPLE_RATE as usize / 10;
    let padded: Vec<f32>;
    let samples = if samples.len() < min_len {
        padded = samples
            .iter()
            .copied()
            .chain(std::iter::repeat(0.0))
            .take(min_len)
            .collect();
        &padded[..]
    } else {
        samples
    };

    let callbacks = Arc::new(CallbackState {
        cancel: cancel.flag(),
        progress: AtomicI32::new(0),
    });
    let language = language.to_string();
    let threads = threads.max(1) as i32;

    let result = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let mut params = FullParams::new(SamplingStrategy::BeamSearch {
                beam_size: 5,
                patience: -1.0,
            });
            params.set_language(Some(language.as_str()));
            params.set_detect_language(false);
            params.set_translate(false);
            params.set_n_threads(threads);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            // Don't emit [MUSIC]/[BLANK_AUDIO]-style annotations or blank segments as "speech".
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);
            let user_data = Arc::as_ptr(&callbacks) as *mut c_void;
            // SAFETY: `callbacks` is kept alive by `recognize` until after this worker has been joined, and
            // the trampolines only read atomics through it.
            unsafe {
                params.set_progress_callback(Some(progress_trampoline));
                params.set_progress_callback_user_data(user_data);
                params.set_abort_callback(Some(abort_trampoline));
                params.set_abort_callback_user_data(user_data);
            }
            state.full(params, samples)
        });

        let mut last = -1;
        while !worker.is_finished() {
            std::thread::sleep(Duration::from_millis(100));
            let p = callbacks.progress.load(Ordering::Relaxed).clamp(0, 100);
            if p != last {
                last = p;
                on_progress(p as u8);
            }
        }
        worker.join()
    });

    let outcome = match result {
        Ok(r) => r,
        Err(_) => return Err(Error::Whisper("the recognition thread panicked".into())),
    };
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    outcome.map_err(|e| Error::Whisper(e.to_string()))?;
    on_progress(100);

    let mut segments = Vec::new();
    for seg in state.as_iter() {
        let text = seg
            .to_str_lossy()
            .map_err(|e| Error::Whisper(format!("reading a result segment: {e}")))?;
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        segments.push(Segment {
            // whisper reports centiseconds.
            start_ms: seg.start_timestamp() * 10,
            end_ms: seg.end_timestamp() * 10,
            text: text.to_string(),
            speaker: None,
        });
    }

    let language = if language == "auto" {
        whisper_rs::get_lang_str(state.full_lang_id_from_state())
            .unwrap_or("auto")
            .to_string()
    } else {
        language
    };
    Ok(Recognition { segments, language })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_only_models_are_recognised_by_name() {
        for m in [
            "tiny.en",
            "base.en-q5_1",
            "small.en-q8_0",
            "medium.en",
            r"C:\m\ggml-base.en.bin",
            "/m/ggml-tiny.en-q5_1.bin",
        ] {
            assert!(is_english_only(m), "{m}");
        }
        for m in [
            "tiny",
            "large-v3-turbo",
            "base-q5_1",
            "medium-q5_0",
            "ggml-tiny.bin",
            "english-ish",
        ] {
            assert!(!is_english_only(m), "{m}");
        }
    }

    #[test]
    fn language_resolution() {
        assert_eq!(resolve_language("large-v3", "auto").unwrap(), "auto");
        assert_eq!(resolve_language("large-v3", "").unwrap(), "auto");
        assert_eq!(resolve_language("large-v3", " DE ").unwrap(), "de");
        assert_eq!(resolve_language("tiny.en", "auto").unwrap(), "en");
        assert_eq!(resolve_language("tiny.en", "en").unwrap(), "en");
        assert!(matches!(
            resolve_language("tiny.en", "de"),
            Err(Error::EnglishOnlyModel { .. })
        ));
        assert!(matches!(
            resolve_language("large-v3", "klingon"),
            Err(Error::UnknownLanguage(_))
        ));
    }
}

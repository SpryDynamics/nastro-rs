//! Audio capture helper for nastro.
//!
//! Spawned by `nastro` (see `nastro-record::TapProcess`), communicating over
//! stdio: `level sys=.. mic=..` lines on stdout once a second, diagnostics on
//! stderr, and the 0 (clean stop) / 2 (permission denied) / 1 (other failure)
//! exit-code contract.
//!
//! Captures real audio via `nastro-audio` (WASAPI on Windows, PulseAudio/PipeWire
//! on Linux), mixes system + mic on a wall-clock schedule, and writes it
//! incrementally to a WAV file (see `wav.rs` - an interim format pending Phase
//! 4's AAC/M4A muxing) alongside a `metadata.json` duration sidecar, matching the
//! `nastro-records` scan contract.

mod wav;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use nastro_audio::{AudioRingBuffer, CaptureError, SampleMixer, TARGET_SAMPLE_RATE};

/// Ring buffer capacity: a few seconds of headroom at the target rate, so a
/// momentarily slow mixer tick doesn't lose samples.
const RING_BUFFER_SECONDS: usize = 5;
const TICK_INTERVAL: Duration = Duration::from_millis(20);
const LEVEL_REPORT_INTERVAL: Duration = Duration::from_secs(1);
/// `metadata.json` is rewritten on this cadence, matching the original macOS
/// project's own 5s interval (cheap, low-value to do more often).
const METADATA_FLUSH_INTERVAL: Duration = Duration::from_secs(5);
/// The WAV header (see `wav.rs`) needs more frequent updates than metadata.json:
/// unlike the fragmented-MP4 approach planned for Phase 4, a RIFF header's
/// declared data size is what makes the *entire file* playable, so this bounds
/// how much a hard kill can cost to "last second of audio" rather than "the
/// whole recording looks like 0 bytes of data".
const WAV_HEADER_FLUSH_INTERVAL: Duration = LEVEL_REPORT_INTERVAL;

const EXIT_CLEAN: i32 = 0;
const EXIT_PERMISSION_DENIED: i32 = 2;
const EXIT_FAILED: i32 = 1;

#[derive(Parser)]
#[command(name = "nastro-tap", about = "Audio capture helper for nastro.")]
struct Args {
    /// Path to write the recording to.
    audio_path: PathBuf,

    #[arg(long, conflicts_with = "system_only")]
    mic_only: bool,

    #[arg(long, conflicts_with = "mic_only")]
    system_only: bool,
}

fn main() {
    let args = Args::parse();
    std::process::exit(run(args));
}

fn run(args: Args) -> i32 {
    let want_system = !args.mic_only;
    let want_mic = !args.system_only;

    let sys_ring = Arc::new(AudioRingBuffer::new(
        TARGET_SAMPLE_RATE as usize * RING_BUFFER_SECONDS,
    ));
    let mic_ring = Arc::new(AudioRingBuffer::new(
        TARGET_SAMPLE_RATE as usize * RING_BUFFER_SECONDS,
    ));

    let system_capture = if want_system {
        match nastro_audio::start_system_capture(Arc::clone(&sys_ring)) {
            Ok(handle) => Some(handle),
            Err(err) => return handle_capture_start_error("system", &err),
        }
    } else {
        None
    };

    let mic_capture = if want_mic {
        match nastro_audio::start_mic_capture(Arc::clone(&mic_ring)) {
            Ok(handle) => Some(handle),
            Err(err) => return handle_capture_start_error("mic", &err),
        }
    } else {
        None
    };

    let mut writer = match wav::WavWriter::create(&args.audio_path, TARGET_SAMPLE_RATE, 1) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Error: failed to create {}: {e}", args.audio_path.display());
            return EXIT_FAILED;
        }
    };

    let stop_flag = Arc::new(AtomicBool::new(false));
    let handler_stop_flag = Arc::clone(&stop_flag);
    if let Err(e) = ctrlc::set_handler(move || {
        handler_stop_flag.store(true, Ordering::SeqCst);
    }) {
        eprintln!("Warning: failed to install stop handler: {e}");
    }

    let exit_code = capture_loop(
        &sys_ring,
        &mic_ring,
        &mut writer,
        &stop_flag,
        &args.audio_path,
    );

    // Order matters: stop capture threads before the final header flush so no
    // more samples arrive mid-finalize.
    drop(system_capture);
    drop(mic_capture);
    let _ = writer.finalize();
    write_metadata(&args.audio_path, metadata_duration(&args.audio_path));

    exit_code
}

fn handle_capture_start_error(label: &str, err: &CaptureError) -> i32 {
    let message = err.to_string();
    eprintln!("Error: {label} capture failed to start: {message}");

    let lower = message.to_lowercase();
    if lower.contains("access") || lower.contains("denied") || lower.contains("permission") {
        EXIT_PERMISSION_DENIED
    } else {
        EXIT_FAILED
    }
}

fn capture_loop(
    sys_ring: &Arc<AudioRingBuffer>,
    mic_ring: &Arc<AudioRingBuffer>,
    writer: &mut wav::WavWriter,
    stop_flag: &Arc<AtomicBool>,
    audio_path: &std::path::Path,
) -> i32 {
    let start = Instant::now();
    let mut mixer = SampleMixer::new(TARGET_SAMPLE_RATE, start);

    let mut sys_level_peak: f32 = 0.0;
    let mut mic_level_peak: f32 = 0.0;
    let mut last_level_report = start;
    let mut last_header_flush = start;
    let mut last_metadata_flush = start;

    while !stop_flag.load(Ordering::SeqCst) {
        std::thread::sleep(TICK_INTERVAL);
        let now = Instant::now();

        let frames = mixer.frames_owed(now);
        if frames > 0 {
            let (mixed, sys_level, mic_level) = mixer.mix_with_levels(sys_ring, mic_ring, frames);
            sys_level_peak = sys_level_peak.max(sys_level);
            mic_level_peak = mic_level_peak.max(mic_level);

            if let Err(e) = writer.write_samples(&mixed) {
                eprintln!("Error: failed writing audio: {e}");
                return EXIT_FAILED;
            }
        }

        if now.duration_since(last_level_report) >= LEVEL_REPORT_INTERVAL {
            println!("level sys={sys_level_peak:.2} mic={mic_level_peak:.2}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            sys_level_peak = 0.0;
            mic_level_peak = 0.0;
            last_level_report = now;
        }

        if now.duration_since(last_header_flush) >= WAV_HEADER_FLUSH_INTERVAL {
            if let Err(e) = writer.flush_header() {
                eprintln!("Warning: failed to flush WAV header: {e}");
            }
            last_header_flush = now;
        }

        if now.duration_since(last_metadata_flush) >= METADATA_FLUSH_INTERVAL {
            write_metadata(audio_path, writer.duration_seconds());
            last_metadata_flush = now;
        }
    }

    EXIT_CLEAN
}

fn metadata_duration(audio_path: &std::path::Path) -> f64 {
    // Recomputed from the just-finalized WAV header via its own duration
    // tracking isn't available post-move; instead, metadata.json is kept in
    // sync during the loop and this final call just re-uses the last known
    // value by reading the file size back (good enough: the WAV header we just
    // wrote already has the authoritative sample count).
    let Ok(bytes) = std::fs::read(audio_path) else {
        return 0.0;
    };
    if bytes.len() < 44 {
        return 0.0;
    }
    let data_size = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as u64;
    let frames = data_size / 2; // 16-bit mono
    frames as f64 / TARGET_SAMPLE_RATE as f64
}

fn write_metadata(audio_path: &std::path::Path, duration_seconds: f64) {
    let Some(dir) = audio_path.parent() else {
        return;
    };
    let metadata_path = dir.join("metadata.json");
    let contents = serde_json::json!({ "duration_seconds": duration_seconds }).to_string();
    let _ = std::fs::write(metadata_path, contents);
}

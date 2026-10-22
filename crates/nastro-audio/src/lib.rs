//! Audio capture/mixing core (placeholder - fleshed out in a later phase).
//!
//! Will host the platform-agnostic ring buffer / wall-clock-driven mixer / rate-drift
//! watcher (ported from `nastro-tap`'s Swift `SampleMixer`/`AudioRingBuffer`/
//! `RateWatcher`), plus platform-specific capture backends: WASAPI loopback on
//! Windows, PulseAudio/PipeWire monitor-source capture on Linux.

pub fn placeholder() -> &'static str {
    "nastro-audio: scaffolded, not yet implemented"
}

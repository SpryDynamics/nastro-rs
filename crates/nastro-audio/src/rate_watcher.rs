//! Sample-rate drift detection, ported from the original Swift `RateWatcher`.
//!
//! Background: the original project's own postmortem (`docs/stack.md` in the
//! upstream Go/Swift repo) found that a capture API's *declared* format sample
//! rate can't always be trusted - measured 48kHz vs. a real 44.1kHz device caused
//! silent buffer underrun and corrupted transcription over a 50-minute recording.
//! `RateWatcher` measures the *actual* delivered-frames/elapsed-time ratio over a
//! rolling window and, if it disagrees with the assumed rate by more than 2% for
//! two consecutive windows, snaps to the nearest standard rate instead of silently
//! drifting.
//!
//! WASAPI's shared-mode `autoconvert` (used by the Windows backend) makes this
//! largely moot in practice, but the watcher is kept as a defensive safety net for
//! misbehaving drivers/backends, matching the original design.

use std::time::{Duration, Instant};

/// Common hardware sample rates to snap to - matches the original's "standard
/// rate" list.
const STANDARD_RATES: &[f64] = &[
    8_000.0, 11_025.0, 16_000.0, 22_050.0, 24_000.0, 32_000.0, 44_100.0, 48_000.0, 88_200.0,
    96_000.0, 176_400.0, 192_000.0,
];

/// Relative drift threshold before a window counts as "disagreeing".
const DRIFT_THRESHOLD: f64 = 0.02;

/// Consecutive disagreeing windows required before snapping.
const DRIFT_WINDOWS_BEFORE_SNAP: u32 = 2;

pub struct RateWatcher {
    assumed_rate: f64,
    window: Duration,
    window_start: Instant,
    frames_in_window: u64,
    consecutive_drift_windows: u32,
}

impl RateWatcher {
    pub fn new(assumed_rate: f64, window: Duration, start: Instant) -> Self {
        Self {
            assumed_rate,
            window,
            window_start: start,
            frames_in_window: 0,
            consecutive_drift_windows: 0,
        }
    }

    pub fn assumed_rate(&self) -> f64 {
        self.assumed_rate
    }

    /// Record that `frames` more frames were delivered, as of wall-clock `now`.
    /// Returns `Some(new_rate)` if a drift-correction snap just occurred.
    pub fn record_frames(&mut self, frames: u64, now: Instant) -> Option<f64> {
        self.frames_in_window += frames;

        let elapsed = now.duration_since(self.window_start);
        if elapsed < self.window {
            return None;
        }

        let measured_rate = self.frames_in_window as f64 / elapsed.as_secs_f64();
        self.window_start = now;
        self.frames_in_window = 0;

        let relative_drift = (measured_rate - self.assumed_rate).abs() / self.assumed_rate;

        if relative_drift <= DRIFT_THRESHOLD {
            self.consecutive_drift_windows = 0;
            return None;
        }

        self.consecutive_drift_windows += 1;
        if self.consecutive_drift_windows < DRIFT_WINDOWS_BEFORE_SNAP {
            return None;
        }

        let snapped = nearest_standard_rate(measured_rate);
        self.assumed_rate = snapped;
        self.consecutive_drift_windows = 0;
        Some(snapped)
    }
}

fn nearest_standard_rate(measured: f64) -> f64 {
    STANDARD_RATES
        .iter()
        .copied()
        .min_by(|a, b| {
            (a - measured)
                .abs()
                .partial_cmp(&(b - measured).abs())
                .unwrap()
        })
        .unwrap_or(measured)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn stable_rate_never_snaps() {
        let start = Instant::now();
        let mut watcher = RateWatcher::new(48_000.0, secs(1), start);

        for i in 1..=10u64 {
            let now = start + secs(i);
            let result = watcher.record_frames(48_000, now);
            assert_eq!(result, None);
        }
        assert_eq!(watcher.assumed_rate(), 48_000.0);
    }

    #[test]
    fn single_drifting_window_does_not_snap_yet() {
        let start = Instant::now();
        let mut watcher = RateWatcher::new(48_000.0, secs(1), start);

        // 44100 vs assumed 48000 is an ~8.1% drift - well over the 2% threshold.
        let result = watcher.record_frames(44_100, start + secs(1));
        assert_eq!(result, None);
        assert_eq!(watcher.assumed_rate(), 48_000.0);
    }

    #[test]
    fn two_consecutive_drifting_windows_snap_to_nearest_standard_rate() {
        let start = Instant::now();
        let mut watcher = RateWatcher::new(48_000.0, secs(1), start);

        assert_eq!(watcher.record_frames(44_100, start + secs(1)), None);
        let result = watcher.record_frames(44_100, start + secs(2));

        assert_eq!(result, Some(44_100.0));
        assert_eq!(watcher.assumed_rate(), 44_100.0);
    }

    #[test]
    fn a_non_drifting_window_resets_the_drift_counter() {
        let start = Instant::now();
        let mut watcher = RateWatcher::new(48_000.0, secs(1), start);

        // One drifting window...
        assert_eq!(watcher.record_frames(44_100, start + secs(1)), None);
        // ...then a stable one resets the counter...
        assert_eq!(watcher.record_frames(48_000, start + secs(2)), None);
        // ...so a single subsequent drifting window still doesn't snap.
        assert_eq!(watcher.record_frames(44_100, start + secs(3)), None);
        assert_eq!(watcher.assumed_rate(), 48_000.0);
    }

    #[test]
    fn snaps_to_the_closest_standard_rate_not_just_the_measured_value() {
        let start = Instant::now();
        let mut watcher = RateWatcher::new(48_000.0, secs(1), start);

        // ~96000-ish measured rate (close to, but not exactly, the standard 96000).
        assert_eq!(watcher.record_frames(95_800, start + secs(1)), None);
        let result = watcher.record_frames(95_800, start + secs(2));

        assert_eq!(result, Some(96_000.0));
    }

    #[test]
    fn does_not_evaluate_before_the_window_elapses() {
        let start = Instant::now();
        let mut watcher = RateWatcher::new(48_000.0, secs(1), start);

        // Half the window has passed - too early to judge drift.
        let result = watcher.record_frames(1_000, start + Duration::from_millis(500));
        assert_eq!(result, None);
    }

    #[test]
    fn after_snapping_the_new_rate_is_treated_as_the_new_baseline() {
        let start = Instant::now();
        let mut watcher = RateWatcher::new(48_000.0, secs(1), start);

        assert_eq!(watcher.record_frames(44_100, start + secs(1)), None);
        assert_eq!(
            watcher.record_frames(44_100, start + secs(2)),
            Some(44_100.0)
        );

        // Now delivering at the new rate should look stable, not drifting again.
        let result = watcher.record_frames(44_100, start + secs(3));
        assert_eq!(result, None);
    }
}

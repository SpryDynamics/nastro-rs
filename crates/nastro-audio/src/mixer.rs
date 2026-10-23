//! Wall-clock-driven sample mixer, ported from the original Swift `SampleMixer`.
//!
//! Rather than draining a fixed number of frames per timer tick (which drifts
//! against real elapsed time), the mixer computes how many frames *ought* to have
//! been produced by now given the target sample rate, carrying any fractional
//! leftover forward so rounding never accumulates into audible drift over a long
//! recording.

use std::time::Instant;

use crate::level::compute_level;
use crate::ring_buffer::AudioRingBuffer;

pub struct SampleMixer {
    sample_rate: f64,
    last_tick: Instant,
    owed_fraction: f64,
}

impl SampleMixer {
    pub fn new(sample_rate: u32, start: Instant) -> Self {
        Self {
            sample_rate: sample_rate as f64,
            last_tick: start,
            owed_fraction: 0.0,
        }
    }

    /// How many whole frames are "owed" since the last call, given wall-clock
    /// `now`. Fractional frames carry over between calls so the running average
    /// rate stays exact even though each call only returns a whole-number count.
    pub fn frames_owed(&mut self, now: Instant) -> usize {
        let elapsed = now.saturating_duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;

        let exact = elapsed * self.sample_rate + self.owed_fraction;
        let whole = exact.floor().max(0.0);
        self.owed_fraction = exact - whole;
        whole as usize
    }

    /// Mix exactly `frames` samples from `sys` and `mic`, zero-filling whichever
    /// source doesn't have enough buffered (silence, or a source that's inactive
    /// in mic-only/system-only mode). Combined amplitude is clamped to `[-1, 1]`
    /// to avoid harsh digital clipping when both sources are loud simultaneously.
    pub fn mix(&self, sys: &AudioRingBuffer, mic: &AudioRingBuffer, frames: usize) -> Vec<f32> {
        self.mix_with_levels(sys, mic, frames).0
    }

    /// Same as [`Self::mix`], but also returns the per-source RMS level of the
    /// samples that were just drained (pre-mix), for `level sys=.. mic=..`
    /// stdout reporting.
    pub fn mix_with_levels(
        &self,
        sys: &AudioRingBuffer,
        mic: &AudioRingBuffer,
        frames: usize,
    ) -> (Vec<f32>, f32, f32) {
        let sys_samples = sys.drain(frames);
        let mic_samples = mic.drain(frames);

        let sys_level = compute_level(&sys_samples);
        let mic_level = compute_level(&mic_samples);

        let mixed = (0..frames)
            .map(|i| {
                let s = sys_samples.get(i).copied().unwrap_or(0.0);
                let m = mic_samples.get(i).copied().unwrap_or(0.0);
                (s + m).clamp(-1.0, 1.0)
            })
            .collect();

        (mixed, sys_level, mic_level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn frames_owed_is_zero_for_zero_elapsed_time() {
        let start = Instant::now();
        let mut mixer = SampleMixer::new(48_000, start);

        assert_eq!(mixer.frames_owed(start), 0);
    }

    #[test]
    fn frames_owed_computes_whole_frames_for_exact_second() {
        let start = Instant::now();
        let mut mixer = SampleMixer::new(48_000, start);

        assert_eq!(mixer.frames_owed(start + Duration::from_secs(1)), 48_000);
    }

    #[test]
    fn frames_owed_carries_fractional_leftover_across_ticks() {
        let start = Instant::now();
        // 2 Hz with 250ms ticks: 0.5 frames owed per tick. Both 2.0 and 0.25 are
        // exactly representable in binary floating point, so this test checks the
        // carry-forward logic itself without also fighting f64 rounding noise from
        // non-exact decimals (e.g. 0.3s ticks, which looked like a good idea once
        // and then produced an off-by-one from pure FP accumulation error).
        let mut mixer = SampleMixer::new(2, start);
        let tick = Duration::from_millis(250);

        let mut total = 0usize;
        let mut now = start;
        let mut per_tick_counts = Vec::new();
        for _ in 0..20 {
            now += tick;
            let owed = mixer.frames_owed(now);
            per_tick_counts.push(owed);
            total += owed;
        }

        // Over 20 ticks of 250ms (5.0s total) at 2 Hz, exactly 10 frames are owed -
        // fractional accumulation must not lose or gain frames over many ticks.
        assert_eq!(total, 10);
        // And it should alternate 0,1,0,1,... rather than e.g. clumping all the
        // owed frames at the start or end.
        assert_eq!(
            per_tick_counts,
            vec![0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]
        );
    }

    #[test]
    fn mix_sums_both_sources() {
        let sys = AudioRingBuffer::new(100);
        let mic = AudioRingBuffer::new(100);
        sys.push(&[0.1, 0.2, 0.3]);
        mic.push(&[0.05, 0.05, 0.05]);

        let mixer = SampleMixer::new(48_000, Instant::now());
        let out = mixer.mix(&sys, &mic, 3);

        assert!((out[0] - 0.15).abs() < 1e-6);
        assert!((out[1] - 0.25).abs() < 1e-6);
        assert!((out[2] - 0.35).abs() < 1e-6);
    }

    #[test]
    fn mix_zero_fills_missing_source_mic_only_mode() {
        let sys = AudioRingBuffer::new(100); // never written to - "system" inactive
        let mic = AudioRingBuffer::new(100);
        mic.push(&[0.4, 0.5]);

        let mixer = SampleMixer::new(48_000, Instant::now());
        let out = mixer.mix(&sys, &mic, 2);

        assert_eq!(out, vec![0.4, 0.5]);
    }

    #[test]
    fn mix_zero_fills_missing_source_system_only_mode() {
        let sys = AudioRingBuffer::new(100);
        sys.push(&[0.4, 0.5]);
        let mic = AudioRingBuffer::new(100); // never written to - "mic" inactive

        let mixer = SampleMixer::new(48_000, Instant::now());
        let out = mixer.mix(&sys, &mic, 2);

        assert_eq!(out, vec![0.4, 0.5]);
    }

    #[test]
    fn mix_always_returns_exactly_the_requested_frame_count() {
        let sys = AudioRingBuffer::new(100);
        let mic = AudioRingBuffer::new(100);
        sys.push(&[0.1]); // fewer samples than requested

        let mixer = SampleMixer::new(48_000, Instant::now());
        let out = mixer.mix(&sys, &mic, 5);

        assert_eq!(out.len(), 5);
    }

    #[test]
    fn mix_clamps_combined_amplitude_to_avoid_clipping_overflow() {
        let sys = AudioRingBuffer::new(100);
        let mic = AudioRingBuffer::new(100);
        sys.push(&[0.8]);
        mic.push(&[0.8]);

        let mixer = SampleMixer::new(48_000, Instant::now());
        let out = mixer.mix(&sys, &mic, 1);

        assert_eq!(out[0], 1.0);
    }

    #[test]
    fn mix_with_levels_reports_per_source_rms_pre_mix() {
        let sys = AudioRingBuffer::new(100);
        let mic = AudioRingBuffer::new(100);
        sys.push(&[0.5, 0.5]);
        mic.push(&[0.25, 0.25]);

        let mixer = SampleMixer::new(48_000, Instant::now());
        let (mixed, sys_level, mic_level) = mixer.mix_with_levels(&sys, &mic, 2);

        assert_eq!(mixed, vec![0.75, 0.75]);
        assert!((sys_level - 0.5).abs() < 1e-6);
        assert!((mic_level - 0.25).abs() < 1e-6);
    }

    #[test]
    fn mix_clamps_negative_overflow_too() {
        let sys = AudioRingBuffer::new(100);
        let mic = AudioRingBuffer::new(100);
        sys.push(&[-0.8]);
        mic.push(&[-0.8]);

        let mixer = SampleMixer::new(48_000, Instant::now());
        let out = mixer.mix(&sys, &mic, 1);

        assert_eq!(out[0], -1.0);
    }
}

//! Discard-on-too-short rule, ported from the Go `internal/record` package's
//! `ShouldDiscard`: recordings under 2 seconds are treated as accidental
//! start/stops and discarded rather than saved.

pub const MIN_KEEPABLE_SECONDS: f64 = 2.0;

pub fn should_discard(duration_seconds: f64) -> bool {
    duration_seconds < MIN_KEEPABLE_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discards_under_threshold() {
        assert!(should_discard(0.0));
        assert!(should_discard(1.9));
    }

    #[test]
    fn keeps_at_or_above_threshold() {
        assert!(!should_discard(2.0));
        assert!(!should_discard(2.1));
        assert!(!should_discard(3600.0));
    }

    #[test]
    fn keeps_negative_duration_is_still_discarded() {
        // Defensive: a negative duration shouldn't happen, but if it does, treat it
        // like "too short" rather than panicking or behaving unpredictably.
        assert!(should_discard(-1.0));
    }
}

//! RMS level computation for the `level sys=.. mic=..` stdout reporting contract
//! consumed by `nastro-record::parse_level_line`.

/// Root-mean-square amplitude of a chunk of samples, in `[0, 1]` for
/// well-formed `[-1, 1]`-range input. Empty input is silence (`0.0`), not an
/// error/NaN.
pub fn compute_level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_zero() {
        assert_eq!(compute_level(&[0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn empty_input_is_zero_not_nan() {
        assert_eq!(compute_level(&[]), 0.0);
    }

    #[test]
    fn constant_amplitude_equals_its_absolute_value() {
        let level = compute_level(&[0.5, 0.5, 0.5, 0.5]);
        assert!((level - 0.5).abs() < 1e-6);
    }

    #[test]
    fn full_scale_square_wave_is_full_scale_rms() {
        let level = compute_level(&[1.0, -1.0, 1.0, -1.0]);
        assert!((level - 1.0).abs() < 1e-6);
    }

    #[test]
    fn known_rms_of_mixed_values() {
        // RMS of [0.0, 1.0] = sqrt((0 + 1) / 2) = sqrt(0.5)
        let level = compute_level(&[0.0, 1.0]);
        assert!((level - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }
}

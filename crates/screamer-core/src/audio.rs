pub const TARGET_SAMPLE_RATE: u32 = 16_000;

pub fn resample_to_target(input: &[f32], from_rate: u32) -> Vec<f32> {
    resample(input, from_rate, TARGET_SAMPLE_RATE)
}

/// Linear interpolation resampler.
fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (input.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = (src_idx - idx as f64) as f32;

        let sample = if idx + 1 < input.len() {
            input[idx] * (1.0 - frac) + input[idx + 1] * frac
        } else if idx < input.len() {
            input[idx]
        } else {
            0.0
        };

        output.push(sample);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_identity() {
        let input = vec![1.0, 2.0, 3.0, 4.0];
        let output = resample(&input, 16_000, 16_000);
        assert_eq!(input, output);
    }

    #[test]
    fn resample_empty() {
        let output = resample(&[], 48_000, 16_000);
        assert!(output.is_empty());
    }

    #[test]
    fn resample_downsample_3x() {
        let input: Vec<f32> = (0..48).map(|i| i as f32).collect();
        let output = resample(&input, 48_000, 16_000);
        assert_eq!(output.len(), 16);
        assert!((output[0] - 0.0).abs() < 0.01);
        assert!((output[1] - 3.0).abs() < 0.01);
    }

    #[test]
    fn resample_upsample_2x() {
        let input = vec![0.0, 1.0, 2.0, 3.0];
        let output = resample(&input, 16_000, 32_000);
        assert_eq!(output.len(), 8);
        assert!((output[0] - 0.0).abs() < 0.01);
        assert!((output[1] - 0.5).abs() < 0.01);
        assert!((output[2] - 1.0).abs() < 0.01);
    }

    #[test]
    fn resample_preserves_approximate_length() {
        let input: Vec<f32> = vec![0.0; 48_000];
        let output = resample(&input, 48_000, 16_000);
        assert!((output.len() as i32 - 16_000).abs() <= 1);
    }

    #[test]
    fn resample_interpolation_accuracy() {
        let input: Vec<f32> = (0..100).map(|i| i as f32 / 99.0).collect();
        let output = resample(&input, 100, 50);
        for i in 1..output.len() {
            assert!(
                output[i] >= output[i - 1],
                "output should be monotonically increasing"
            );
        }
    }
}

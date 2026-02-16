use super::BeamSample;

/// Resample beam path by arc length to decouple energy deposition rate from
/// input sample rate.
///
/// At high sample rates, consecutive samples are closer together than the beam
/// radius, creating visible periodic brightness modulation along traces. This
/// function merges short segments into longer ones, spacing depositions at
/// approximately `threshold` distance (in normalized screen coordinates).
///
/// Energy is conserved: each merged segment's `intensity * dt` equals the sum
/// of the constituent samples' `intensity * dt` products. The first lit sample
/// in each run is emitted directly (as a line-start anchor for the shader);
/// subsequent depositions emit when accumulated arc length exceeds the threshold.
/// Any remaining energy is flushed at the end.
pub fn arc_length_resample(samples: &[BeamSample], threshold: f32) -> Vec<BeamSample> {
    if samples.is_empty() || threshold <= 0.0 {
        return samples.to_vec();
    }

    let mut output = Vec::with_capacity(samples.len());

    let mut prev_x: f32 = 0.0;
    let mut prev_y: f32 = 0.0;
    let mut accum_energy: f32 = 0.0;
    let mut accum_dist: f32 = 0.0;
    let mut in_run = false;

    for &sample in samples {
        if sample.intensity <= 0.0 {
            // Flush any pending energy before the blank
            if in_run && accum_energy > 0.0 {
                output.push(BeamSample {
                    x: prev_x,
                    y: prev_y,
                    intensity: accum_energy,
                    dt: 1.0,
                });
            }
            // Emit blank as-is (retrace marker)
            output.push(sample);
            accum_energy = 0.0;
            accum_dist = 0.0;
            in_run = false;
            continue;
        }

        if !in_run {
            // First lit sample in a new run — emit directly so the shader has
            // a starting point to form line segments from.
            output.push(sample);
            prev_x = sample.x;
            prev_y = sample.y;
            accum_energy = 0.0;
            accum_dist = 0.0;
            in_run = true;
            continue;
        }

        // Accumulate arc length and energy
        let dx = sample.x - prev_x;
        let dy = sample.y - prev_y;
        accum_dist += (dx * dx + dy * dy).sqrt();
        accum_energy += sample.intensity * sample.dt;
        prev_x = sample.x;
        prev_y = sample.y;

        if accum_dist >= threshold {
            output.push(BeamSample {
                x: sample.x,
                y: sample.y,
                intensity: accum_energy,
                dt: 1.0,
            });
            accum_energy = 0.0;
            accum_dist = 0.0;
        }
    }

    // End-of-frame flush: deposit any remaining accumulated energy
    if in_run && accum_energy > 0.0 {
        output.push(BeamSample {
            x: prev_x,
            y: prev_y,
            intensity: accum_energy,
            dt: 1.0,
        });
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(x: f32, y: f32, intensity: f32, dt: f32) -> BeamSample {
        BeamSample {
            x,
            y,
            intensity,
            dt,
        }
    }

    #[test]
    fn empty_input() {
        assert!(arc_length_resample(&[], 0.1).is_empty());
    }

    #[test]
    fn single_sample_unchanged() {
        let input = [sample(0.5, 0.5, 1.0, 0.001)];
        let output = arc_length_resample(&input, 0.1);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].x, 0.5);
        assert_eq!(output[0].intensity, 1.0);
        assert_eq!(output[0].dt, 0.001);
    }

    #[test]
    fn zero_threshold_passthrough() {
        let input = [sample(0.1, 0.5, 1.0, 0.001), sample(0.2, 0.5, 1.0, 0.001)];
        let output = arc_length_resample(&input, 0.0);
        assert_eq!(output.len(), 2);
    }

    #[test]
    fn close_samples_merged() {
        // 10 samples each 0.01 apart along X, threshold 0.05
        let input: Vec<_> = (0..10)
            .map(|i| sample(0.5 + i as f32 * 0.01, 0.5, 1.0, 0.001))
            .collect();
        let output = arc_length_resample(&input, 0.05);
        // First sample emitted directly, then depositions ~every 5 samples,
        // plus possibly a flush. Should be significantly fewer than 10.
        assert!(output.len() < input.len());
        assert!(output.len() >= 2);
    }

    #[test]
    fn energy_conserved() {
        let input: Vec<_> = (0..100)
            .map(|i| sample(0.5 + i as f32 * 0.001, 0.5, 2.0, 0.001))
            .collect();
        let total_input: f32 = input.iter().map(|s| s.intensity * s.dt).sum();
        let output = arc_length_resample(&input, 0.01);
        let total_output: f32 = output.iter().map(|s| s.intensity * s.dt).sum();
        assert!(
            (total_input - total_output).abs() < 1e-5,
            "input={total_input}, output={total_output}"
        );
    }

    #[test]
    fn blanks_break_runs() {
        let input = [
            sample(0.1, 0.5, 1.0, 0.001),
            sample(0.11, 0.5, 1.0, 0.001),
            sample(0.3, 0.5, 0.0, 0.001), // blank retrace
            sample(0.4, 0.5, 1.0, 0.001),
            sample(0.41, 0.5, 1.0, 0.001),
        ];
        let output = arc_length_resample(&input, 0.5);
        // Blank must be preserved
        assert!(output.iter().any(|s| s.intensity <= 0.0));
        // Should have at least: run1-start, run1-flush, blank, run2-start, run2-flush
        assert!(output.len() >= 3);
    }

    #[test]
    fn far_apart_samples_not_merged() {
        // Samples spaced far apart (> threshold) — each emitted individually
        let input: Vec<_> = (0..5)
            .map(|i| sample(i as f32 * 0.2, 0.5, 1.0, 0.001))
            .collect();
        let output = arc_length_resample(&input, 0.05);
        // First emitted directly, rest each exceed threshold immediately
        assert_eq!(output.len(), input.len());
    }

    #[test]
    fn stationary_beam_flushed() {
        // All samples at the same position — distance never exceeds threshold
        let input: Vec<_> = (0..10).map(|_| sample(0.5, 0.5, 1.0, 0.001)).collect();
        let output = arc_length_resample(&input, 0.01);
        // First sample emitted directly, rest accumulated and flushed as one
        assert_eq!(output.len(), 2);
        // Total energy should be conserved
        let total_input: f32 = input.iter().map(|s| s.intensity * s.dt).sum();
        let total_output: f32 = output.iter().map(|s| s.intensity * s.dt).sum();
        assert!(
            (total_input - total_output).abs() < 1e-6,
            "input={total_input}, output={total_output}"
        );
    }
}

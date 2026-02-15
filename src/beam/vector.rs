use super::{BeamSample, BeamSource, BeamState};

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct VectorSegment {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub intensity: f32,
}

pub struct VectorSource {
    pub segments: Vec<VectorSegment>,
    pub beam_speed: f32,    // units per second (normalized coords)
    pub settling_time: f32, // seconds for retrace between disconnected segments
}

/// Minimum number of subdivisions per segment (even very short ones get at least this many).
const MIN_SUBDIVISIONS: usize = 2;

impl BeamSource for VectorSource {
    fn generate(&mut self, _count: usize, beam: &BeamState) -> Vec<BeamSample> {
        let mut out = Vec::new();
        let mut prev_end: Option<(f32, f32)> = None;

        for seg in &self.segments {
            // Insert blanked retrace if the beam must jump to a new position
            if let Some((px, py)) = prev_end {
                let dx = seg.x0 - px;
                let dy = seg.y0 - py;
                if dx.abs() > 1e-6 || dy.abs() > 1e-6 {
                    out.push(BeamSample {
                        x: seg.x0,
                        y: seg.y0,
                        intensity: 0.0,
                        dt: self.settling_time,
                    });
                }
            }

            // Subdivide so consecutive samples are within one spot radius
            let dx = seg.x1 - seg.x0;
            let dy = seg.y1 - seg.y0;
            let length = (dx * dx + dy * dy).sqrt();
            let steps = ((length / beam.spot_radius).ceil() as usize).max(MIN_SUBDIVISIONS);
            let dt = length / (self.beam_speed * steps as f32);

            for i in 0..steps {
                let t = (i as f32 + 0.5) / steps as f32;
                out.push(BeamSample {
                    x: seg.x0 + dx * t,
                    y: seg.y0 + dy * t,
                    intensity: seg.intensity,
                    dt,
                });
            }

            prev_end = Some((seg.x1, seg.y1));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BEAM: BeamState = BeamState { spot_radius: 0.001 };

    #[test]
    fn single_segment_produces_samples_along_line() {
        let mut src = VectorSource {
            segments: vec![VectorSegment {
                x0: 0.0,
                y0: 0.0,
                x1: 1.0,
                y1: 0.0,
                intensity: 1.0,
            }],
            beam_speed: 1.0,
            settling_time: 0.001,
        };
        let samples = src.generate(0, &TEST_BEAM);
        assert!(!samples.is_empty());
        for s in &samples {
            if s.intensity > 0.0 {
                assert!((s.y).abs() < 0.01);
                assert!(s.x >= 0.0 && s.x <= 1.0);
            }
        }
    }

    #[test]
    fn disconnected_segments_have_blanked_retrace() {
        let mut src = VectorSource {
            segments: vec![
                VectorSegment {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 0.5,
                    y1: 0.0,
                    intensity: 1.0,
                },
                VectorSegment {
                    x0: 0.5,
                    y0: 0.5,
                    x1: 1.0,
                    y1: 0.5,
                    intensity: 1.0,
                },
            ],
            beam_speed: 1.0,
            settling_time: 0.001,
        };
        let samples = src.generate(0, &TEST_BEAM);
        assert!(samples.iter().any(|s| s.intensity == 0.0));
    }

    #[test]
    fn connected_segments_have_no_retrace() {
        let mut src = VectorSource {
            segments: vec![
                VectorSegment {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 0.5,
                    y1: 0.5,
                    intensity: 1.0,
                },
                VectorSegment {
                    x0: 0.5,
                    y0: 0.5,
                    x1: 1.0,
                    y1: 0.0,
                    intensity: 1.0,
                },
            ],
            beam_speed: 1.0,
            settling_time: 0.001,
        };
        let samples = src.generate(0, &TEST_BEAM);
        assert!(samples.iter().all(|s| s.intensity > 0.0));
    }

    #[test]
    fn longer_segments_produce_more_samples() {
        let mut short_src = VectorSource {
            segments: vec![VectorSegment {
                x0: 0.0,
                y0: 0.0,
                x1: 0.1,
                y1: 0.0,
                intensity: 1.0,
            }],
            beam_speed: 1.0,
            settling_time: 0.0,
        };
        let mut long_src = VectorSource {
            segments: vec![VectorSegment {
                x0: 0.0,
                y0: 0.0,
                x1: 1.0,
                y1: 0.0,
                intensity: 1.0,
            }],
            beam_speed: 1.0,
            settling_time: 0.0,
        };
        assert!(long_src.generate(0, &TEST_BEAM).len() > short_src.generate(0, &TEST_BEAM).len());
    }
}

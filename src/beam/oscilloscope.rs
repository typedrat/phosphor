use super::BeamSample;

pub enum Waveform {
    Sine,
    Triangle,
    Square,
    Sawtooth,
}

pub struct ChannelConfig {
    pub waveform: Waveform,
    pub frequency: f32,
    pub amplitude: f32,
    pub phase: f32,
    pub dc_offset: f32,
}

/// Evaluate a waveform at phase `p` (in radians). Returns value in [-1, 1].
fn eval_waveform(waveform: &Waveform, p: f32) -> f32 {
    match waveform {
        Waveform::Sine => p.sin(),
        Waveform::Triangle => {
            // Normalize phase to [0, 2pi)
            let t = p.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
            if t < 0.25 {
                4.0 * t
            } else if t < 0.75 {
                2.0 - 4.0 * t
            } else {
                4.0 * t - 4.0
            }
        }
        Waveform::Square => {
            if p.sin() >= 0.0 {
                1.0
            } else {
                -1.0
            }
        }
        Waveform::Sawtooth => {
            let t = p.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
            2.0 * t - 1.0
        }
    }
}

/// Evaluate a channel config at time `t`, returning a screen coordinate in [0, 1].
fn eval_channel(config: &ChannelConfig, t: f32) -> f32 {
    let phase = std::f32::consts::TAU * config.frequency * t + config.phase;
    let deflection = config.amplitude * eval_waveform(&config.waveform, phase) + config.dc_offset;
    (0.5 + deflection).clamp(0.0, 1.0)
}

pub struct OscilloscopeSource {
    pub x_channel: ChannelConfig,
    pub y_channel: ChannelConfig,
    pub sample_rate: f32,
}

impl OscilloscopeSource {
    /// Generate `count` samples starting at simulated time `t_start`.
    pub fn generate(&self, t_start: f32, count: usize) -> Vec<BeamSample> {
        let dt = 1.0 / self.sample_rate;
        (0..count)
            .map(|i| {
                let t = t_start + i as f32 * dt;
                BeamSample {
                    x: eval_channel(&self.x_channel, t),
                    y: eval_channel(&self.y_channel, t),
                    intensity: 1.0,
                    dt,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_generates_correct_range() {
        let src = OscilloscopeSource {
            x_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 1.0,
                phase: 0.0,
                dc_offset: 0.0,
            },
            y_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 1.0,
                phase: std::f32::consts::FRAC_PI_2,
                dc_offset: 0.0,
            },
            sample_rate: 1000.0,
        };

        let samples = src.generate(0.0, 1000);
        assert_eq!(samples.len(), 1000);

        for s in &samples {
            assert!(s.x >= 0.0 && s.x <= 1.0, "x={} out of range", s.x);
            assert!(s.y >= 0.0 && s.y <= 1.0, "y={} out of range", s.y);
            assert!(s.intensity > 0.0);
        }
    }

    #[test]
    fn sine_cosine_makes_circle() {
        let src = OscilloscopeSource {
            x_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 0.4,
                phase: 0.0,
                dc_offset: 0.0,
            },
            y_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 0.4,
                phase: std::f32::consts::FRAC_PI_2,
                dc_offset: 0.0,
            },
            sample_rate: 1000.0,
        };

        let samples = src.generate(0.0, 1000);
        for s in &samples {
            let dx = s.x - 0.5;
            let dy = s.y - 0.5;
            let r = (dx * dx + dy * dy).sqrt();
            assert!((r - 0.4).abs() < 0.01, "r={r}, expected ~0.4");
        }
    }

    #[test]
    fn square_wave_is_binary() {
        let src = OscilloscopeSource {
            x_channel: ChannelConfig {
                waveform: Waveform::Square,
                frequency: 10.0,
                amplitude: 1.0,
                phase: 0.0,
                dc_offset: 0.0,
            },
            y_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 1.0,
                phase: 0.0,
                dc_offset: 0.0,
            },
            sample_rate: 10000.0,
        };

        let samples = src.generate(0.0, 10000);
        for s in &samples {
            assert!(s.x < 0.01 || s.x > 0.99, "x={} not binary", s.x);
        }
    }

    #[test]
    fn dt_matches_sample_rate() {
        let src = OscilloscopeSource {
            x_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 1.0,
                phase: 0.0,
                dc_offset: 0.0,
            },
            y_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 1.0,
                phase: 0.0,
                dc_offset: 0.0,
            },
            sample_rate: 44100.0,
        };

        let samples = src.generate(0.0, 100);
        for s in &samples {
            assert!((s.dt - 1.0 / 44100.0).abs() < 1e-9);
        }
    }
}

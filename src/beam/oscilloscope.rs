use super::{BeamSample, BeamSource, BeamState};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, strum::Display, strum::EnumIter)]
pub enum Waveform {
    #[default]
    Sine,
    Triangle,
    Square,
    Sawtooth,
}

#[derive(Clone)]
pub struct ChannelConfig {
    pub waveform: Waveform,
    pub frequency: f32,
    pub amplitude: f32,
    pub phase: f32,
    pub dc_offset: f32,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            waveform: Waveform::Sine,
            frequency: 100.0,
            amplitude: 0.4,
            phase: 0.0,
            dc_offset: 0.0,
        }
    }
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
    t_current: f32,
}

impl OscilloscopeSource {
    pub fn new(x_channel: ChannelConfig, y_channel: ChannelConfig, sample_rate: f32) -> Self {
        Self {
            x_channel,
            y_channel,
            sample_rate,
            t_current: 0.0,
        }
    }
}

impl BeamSource for OscilloscopeSource {
    fn generate(&mut self, count: usize, _beam: &BeamState) -> Vec<BeamSample> {
        let dt = 1.0 / self.sample_rate;
        let samples = (0..count)
            .map(|i| {
                let t = self.t_current + i as f32 * dt;
                BeamSample {
                    x: eval_channel(&self.x_channel, t),
                    y: eval_channel(&self.y_channel, t),
                    intensity: 1.0,
                    dt,
                }
            })
            .collect();
        self.t_current += count as f32 * dt;
        samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BEAM: BeamState = BeamState { spot_radius: 0.001 };

    #[test]
    fn sine_generates_correct_range() {
        let mut src = OscilloscopeSource::new(
            ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 1.0,
                phase: 0.0,
                dc_offset: 0.0,
            },
            ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 1.0,
                phase: std::f32::consts::FRAC_PI_2,
                dc_offset: 0.0,
            },
            1000.0,
        );

        let samples = src.generate(1000, &TEST_BEAM);
        assert_eq!(samples.len(), 1000);

        for s in &samples {
            assert!(s.x >= 0.0 && s.x <= 1.0, "x={} out of range", s.x);
            assert!(s.y >= 0.0 && s.y <= 1.0, "y={} out of range", s.y);
            assert!(s.intensity > 0.0);
        }
    }

    #[test]
    fn sine_cosine_makes_circle() {
        let mut src = OscilloscopeSource::new(
            ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 0.4,
                phase: 0.0,
                dc_offset: 0.0,
            },
            ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 0.4,
                phase: std::f32::consts::FRAC_PI_2,
                dc_offset: 0.0,
            },
            1000.0,
        );

        let samples = src.generate(1000, &TEST_BEAM);
        for s in &samples {
            let dx = s.x - 0.5;
            let dy = s.y - 0.5;
            let r = (dx * dx + dy * dy).sqrt();
            assert!((r - 0.4).abs() < 0.01, "r={r}, expected ~0.4");
        }
    }

    #[test]
    fn square_wave_is_binary() {
        let mut src = OscilloscopeSource::new(
            ChannelConfig {
                waveform: Waveform::Square,
                frequency: 10.0,
                amplitude: 1.0,
                phase: 0.0,
                dc_offset: 0.0,
            },
            ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 1.0,
                phase: 0.0,
                dc_offset: 0.0,
            },
            10000.0,
        );

        let samples = src.generate(10000, &TEST_BEAM);
        for s in &samples {
            assert!(s.x < 0.01 || s.x > 0.99, "x={} not binary", s.x);
        }
    }

    #[test]
    fn dt_matches_sample_rate() {
        let mut src = OscilloscopeSource::new(
            ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 1.0,
                phase: 0.0,
                dc_offset: 0.0,
            },
            ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0,
                amplitude: 1.0,
                phase: 0.0,
                dc_offset: 0.0,
            },
            44100.0,
        );

        let samples = src.generate(100, &TEST_BEAM);
        for s in &samples {
            assert!((s.dt - 1.0 / 44100.0).abs() < 1e-9);
        }
    }
}

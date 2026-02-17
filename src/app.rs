use std::path::PathBuf;

use crate::beam::audio::AudioSource;
use crate::beam::oscilloscope::{ChannelConfig, OscilloscopeSource, Waveform};
use crate::beam::vector::VectorSegment;
use crate::beam::{BeamSample, BeamSource, BeamState};

/// Calibration constant for beam energy deposition. The beam_write shader
/// computes `energy = intensity * profile * dt`, where dt is the per-sample
/// dwell time (~1/44100 s). Without scaling, the deposited energy is on the
/// order of 1e-5, which is invisible after spectral integration and
/// tonemapping. This constant represents the beam current / power scale
/// that makes the phosphor visibly glow at the default settings.
const BEAM_ENERGY_SCALE: f32 = 5000.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    #[default]
    Oscilloscope,
    Audio,
    Vector,
    External,
}

#[derive(Clone)]
pub struct OscilloscopeState {
    pub x_waveform: Waveform,
    pub x_frequency: f32,
    pub x_amplitude: f32,
    pub x_phase: f32,
    pub x_dc_offset: f32,
    pub y_waveform: Waveform,
    pub y_frequency: f32,
    pub y_amplitude: f32,
    pub y_phase: f32,
    pub y_dc_offset: f32,
    pub sample_rate: f32,
}

impl Default for OscilloscopeState {
    fn default() -> Self {
        Self {
            x_waveform: Waveform::Sine,
            x_frequency: 100.0,
            x_amplitude: 0.4,
            x_phase: 0.0,
            x_dc_offset: 0.0,
            y_waveform: Waveform::Sine,
            y_frequency: 100.0,
            y_amplitude: 0.4,
            y_phase: std::f32::consts::FRAC_PI_2,
            y_dc_offset: 0.0,
            sample_rate: 44100.0,
        }
    }
}

pub struct AudioState {
    pub file_path: Option<PathBuf>,
    pub source: Option<AudioSource>,
    pub playing: bool,
    pub looping: bool,
    pub speed: f32,
    pub load_error: Option<String>,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            file_path: None,
            source: None,
            playing: false,
            looping: false,
            speed: 1.0,
            load_error: None,
        }
    }
}

pub struct VectorState {
    pub file_path: Option<PathBuf>,
    pub segments: Vec<VectorSegment>,
    pub beam_speed: f32,
    pub settling_time: f32,
    pub looping: bool,
    pub load_error: Option<String>,
}

impl Default for VectorState {
    fn default() -> Self {
        Self {
            file_path: None,
            segments: Vec::new(),
            beam_speed: 1.0,
            settling_time: 0.001,
            looping: true,
            load_error: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExternalMode {
    #[default]
    Stdin,
    Socket,
}

pub struct ExternalState {
    pub mode: ExternalMode,
    pub socket_path: String,
    pub connected: bool,
}

impl Default for ExternalState {
    fn default() -> Self {
        Self {
            mode: ExternalMode::Stdin,
            socket_path: String::new(),
            connected: false,
        }
    }
}

pub struct InputState {
    pub mode: InputMode,
    pub oscilloscope: OscilloscopeState,
    pub audio: AudioState,
    pub vector: VectorState,
    pub external: ExternalState,
    osc_source: OscilloscopeSource,
}

impl Default for InputState {
    fn default() -> Self {
        let osc = OscilloscopeState::default();
        let osc_source = OscilloscopeSource::new(
            ChannelConfig {
                waveform: osc.x_waveform,
                frequency: osc.x_frequency,
                amplitude: osc.x_amplitude,
                phase: osc.x_phase,
                dc_offset: osc.x_dc_offset,
            },
            ChannelConfig {
                waveform: osc.y_waveform,
                frequency: osc.y_frequency,
                amplitude: osc.y_amplitude,
                phase: osc.y_phase,
                dc_offset: osc.y_dc_offset,
            },
            osc.sample_rate,
        );

        Self {
            mode: InputMode::default(),
            oscilloscope: osc,
            audio: AudioState::default(),
            vector: VectorState::default(),
            external: ExternalState::default(),
            osc_source,
        }
    }
}

impl InputState {
    /// Generate a fixed number of samples at the given sample rate.
    /// Unlike `generate_samples`, this does NOT measure wall-clock time —
    /// dt is always `1/sample_rate`, making output deterministic.
    pub fn generate_samples_fixed(
        &mut self,
        focus: f32,
        aspect: f32,
        viewport_width: f32,
        sample_rate: f32,
        count: usize,
    ) -> Vec<BeamSample> {
        let spot_radius = focus / viewport_width.max(1.0);
        let beam = BeamState { spot_radius };

        let mut samples = match self.mode {
            InputMode::Oscilloscope => {
                self.sync_oscilloscope_params();
                self.osc_source.sample_rate = sample_rate;
                if count == 0 {
                    return Vec::new();
                }
                self.osc_source.generate(count, &beam)
            }
            InputMode::Audio => {
                let audio = &mut self.audio;
                if !audio.playing {
                    return Vec::new();
                }
                let Some(source) = &mut audio.source else {
                    return Vec::new();
                };
                let adj_count = (count as f32 * audio.speed) as usize;
                if adj_count == 0 {
                    return Vec::new();
                }
                let samples = source.generate(adj_count, &beam);
                if source.is_finished() {
                    if audio.looping {
                        source.seek(0.0);
                    } else {
                        audio.playing = false;
                    }
                }
                samples
            }
            InputMode::Vector => {
                if self.vector.segments.is_empty() {
                    return Vec::new();
                }
                let mut src = crate::beam::vector::VectorSource {
                    segments: self.vector.segments.clone(),
                    beam_speed: self.vector.beam_speed,
                    settling_time: self.vector.settling_time,
                };
                src.generate(0, &beam)
            }
            InputMode::External => Vec::new(),
        };

        // Aspect ratio correction
        if aspect > 1.0 {
            for s in &mut samples {
                s.x = 0.5 + (s.x - 0.5) / aspect;
            }
        } else if aspect < 1.0 {
            for s in &mut samples {
                s.y = 0.5 + (s.y - 0.5) * aspect;
            }
        }

        // Arc-length resample
        let mut samples = crate::beam::resample::arc_length_resample(&samples, spot_radius * 0.5);

        // Scale beam energy
        for s in &mut samples {
            s.intensity *= BEAM_ENERGY_SCALE;
        }

        samples
    }

    fn sync_oscilloscope_params(&mut self) {
        let osc = &self.oscilloscope;
        self.osc_source.x_channel.waveform = osc.x_waveform;
        self.osc_source.x_channel.frequency = osc.x_frequency;
        self.osc_source.x_channel.amplitude = osc.x_amplitude;
        self.osc_source.x_channel.phase = osc.x_phase;
        self.osc_source.x_channel.dc_offset = osc.x_dc_offset;
        self.osc_source.y_channel.waveform = osc.y_waveform;
        self.osc_source.y_channel.frequency = osc.y_frequency;
        self.osc_source.y_channel.amplitude = osc.y_amplitude;
        self.osc_source.y_channel.phase = osc.y_phase;
        self.osc_source.y_channel.dc_offset = osc.y_dc_offset;
        self.osc_source.sample_rate = osc.sample_rate;
    }

    pub fn load_audio_file(&mut self, path: PathBuf) {
        match AudioSource::load(&path) {
            Ok(source) => {
                self.audio.source = Some(source);
                self.audio.file_path = Some(path);
                self.audio.load_error = None;
                self.audio.playing = true;
            }
            Err(e) => {
                self.audio.load_error = Some(e.to_string());
                self.audio.source = None;
            }
        }
    }

    pub fn load_vector_file(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<Vec<VectorSegment>>(&contents) {
                Ok(segments) => {
                    self.vector.segments = segments;
                    self.vector.file_path = Some(path);
                    self.vector.load_error = None;
                }
                Err(e) => {
                    self.vector.load_error = Some(e.to_string());
                    self.vector.segments.clear();
                }
            },
            Err(e) => {
                self.vector.load_error = Some(e.to_string());
                self.vector.segments.clear();
            }
        }
    }
}

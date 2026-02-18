use crate::beam::oscilloscope::Waveform;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    #[default]
    Oscilloscope,
    Audio,
    Vector,
    External,
}

#[derive(Clone, PartialEq)]
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
        // Circle preset (Lissajous 1:1) — inlined to avoid circular
        // dependency with crate::presets which imports OscilloscopeState.
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

/// Width x height pixel dimensions.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}\u{00d7}{}", self.width, self.height)
    }
}

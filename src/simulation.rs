use std::path::PathBuf;

use crate::app::{InputMode, OscilloscopeState};
use crate::beam::SampleProducer;
use crate::types::Resolution;

/// Commands sent from the render/UI thread to the simulation thread.
pub enum SimCommand {
    SetInputMode(InputMode),
    SetOscilloscopeParams(OscilloscopeState),
    SetFocus(f32),
    /// Viewport dimensions and offset for aspect ratio correction.
    /// `x_offset` is the sidebar width in pixels (0 when hidden or detached).
    SetViewport {
        width: f32,
        height: f32,
        x_offset: f32,
    },
    SetAccumResolution(Resolution),
    LoadAudioFile(PathBuf),
    SetAudioPlaying(bool),
    SetAudioLooping(bool),
    SetAudioSpeed(f32),
    LoadVectorFile(PathBuf),
    /// Sample rate change — carries the new producer from a resized channel.
    /// The render thread creates the new channel and swaps its consumer.
    SetSampleRate {
        rate: f32,
        producer: SampleProducer,
    },
    Shutdown,
}

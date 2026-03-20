use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use atomic_float::AtomicF32;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dasp_interpolate::Interpolator;
use dasp_interpolate::linear::Linear;

/// Decoded audio ready to be shared.
pub struct DecodedAudio {
    pub samples: Arc<[f32]>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Shared playback state coordinating cpal callback, sim thread, and UI.
pub struct SharedAudioPlayback {
    pub samples: Arc<[f32]>,
    pub channels: u16,
    pub sample_rate: u32,
    pub position: AtomicUsize,
    pub seek_target: AtomicUsize,
    pub playing: AtomicBool,
    pub looping: AtomicBool,
    pub speed: AtomicF32,
    pub volume: AtomicF32,
}

impl SharedAudioPlayback {
    pub fn new(decoded: DecodedAudio) -> Self {
        Self {
            samples: decoded.samples,
            channels: decoded.channels,
            sample_rate: decoded.sample_rate,
            position: AtomicUsize::new(0),
            seek_target: AtomicUsize::new(usize::MAX),
            playing: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            speed: AtomicF32::new(1.0),
            volume: AtomicF32::new(1.0),
        }
    }

    pub fn total_frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn duration_secs(&self) -> f32 {
        self.total_frames() as f32 / self.sample_rate as f32
    }

    pub fn position_secs(&self) -> f32 {
        self.position.load(Ordering::Relaxed) as f32 / self.sample_rate as f32
    }

    pub fn seek_to_frame(&self, frame: usize) {
        self.seek_target
            .store(frame.min(self.total_frames()), Ordering::Relaxed);
    }

    pub fn seek_to_secs(&self, secs: f32) {
        let frame = (secs * self.sample_rate as f32) as usize;
        self.seek_to_frame(frame);
    }
}

/// Manages the cpal output stream for audio playback.
pub struct AudioOutput {
    _stream: cpal::Stream,
    pub shared: Arc<SharedAudioPlayback>,
}

impl AudioOutput {
    /// Create a new audio output stream. Returns `Err` if no audio device
    /// is available — the app should degrade to visual-only mode.
    pub fn new(shared: Arc<SharedAudioPlayback>) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("no audio output device found"))?;

        let config = device.default_output_config()?;
        let device_sample_rate = config.sample_rate().0;
        let device_channels = config.channels() as usize;

        tracing::info!(
            device = device.name().unwrap_or_default(),
            sample_rate = device_sample_rate,
            channels = device_channels,
            "Audio output device"
        );

        let shared_cb = Arc::clone(&shared);
        let source_rate = shared.sample_rate;
        let source_channels = shared.channels as usize;

        // Callback-local state
        let mut fractional_pos: f64 = 0.0;
        let mut interp_l = Linear::new([0.0f32], [0.0f32]);
        let mut interp_r = Linear::new([0.0f32], [0.0f32]);
        let mut last_int_pos: usize = usize::MAX; // force initial load

        let stream = device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                // Check for pending seek
                let seek = shared_cb.seek_target.swap(usize::MAX, Ordering::Relaxed);
                if seek != usize::MAX {
                    fractional_pos = seek as f64;
                    shared_cb.position.store(seek, Ordering::Relaxed);
                    last_int_pos = usize::MAX; // force interpolator reload
                }

                if !shared_cb.playing.load(Ordering::Relaxed) {
                    data.fill(0.0);
                    return;
                }

                let speed = shared_cb.speed.load(Ordering::Relaxed) as f64;
                let volume = shared_cb.volume.load(Ordering::Relaxed);
                let advance_rate = (source_rate as f64 * speed) / device_sample_rate as f64;
                let total_frames = shared_cb.total_frames();
                let samples = &shared_cb.samples;

                let output_frames = data.len() / device_channels;
                for frame_idx in 0..output_frames {
                    let int_pos = fractional_pos as usize;

                    if int_pos >= total_frames {
                        if shared_cb.looping.load(Ordering::Relaxed) {
                            fractional_pos = 0.0;
                            last_int_pos = usize::MAX;
                        } else {
                            let start = frame_idx * device_channels;
                            data[start..].fill(0.0);
                            shared_cb.playing.store(false, Ordering::Relaxed);
                            break;
                        }
                    }

                    let pos = fractional_pos as usize;
                    let frac = fractional_pos - pos as f64;

                    // Feed new frames to the interpolators when we cross a frame boundary
                    if pos != last_int_pos {
                        let next_pos = if pos + 1 < total_frames {
                            pos + 1
                        } else if shared_cb.looping.load(Ordering::Relaxed) {
                            0
                        } else {
                            pos
                        };
                        let base = pos * source_channels;
                        let next_base = next_pos * source_channels;
                        interp_l = Linear::new([samples[base]], [samples[next_base]]);
                        interp_r = Linear::new([samples[base + 1]], [samples[next_base + 1]]);
                        last_int_pos = pos;
                    }

                    let [l] = interp_l.interpolate(frac);
                    let [r] = interp_r.interpolate(frac);

                    // Write to output — map stereo to device channels
                    let out_base = frame_idx * device_channels;
                    data[out_base] = l * volume;
                    if device_channels >= 2 {
                        data[out_base + 1] = r * volume;
                    }
                    for ch in 2..device_channels {
                        data[out_base + ch] = 0.0;
                    }

                    fractional_pos += advance_rate;
                }

                shared_cb
                    .position
                    .store(fractional_pos as usize, Ordering::Relaxed);
            },
            |err| {
                tracing::error!("Audio output error: {err}");
            },
            None,
        )?;

        stream.play()?;

        Ok(Self {
            _stream: stream,
            shared,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_shared(num_frames: usize, sample_rate: u32) -> SharedAudioPlayback {
        let channels: u16 = 2;
        let samples: Vec<f32> = vec![0.0; num_frames * channels as usize];
        SharedAudioPlayback::new(DecodedAudio {
            samples: samples.into(),
            sample_rate,
            channels,
        })
    }

    #[test]
    fn total_frames_stereo() {
        let shared = make_shared(1000, 44100);
        assert_eq!(shared.total_frames(), 1000);
    }

    #[test]
    fn duration_secs() {
        let shared = make_shared(44100, 44100);
        assert!((shared.duration_secs() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn position_secs_tracks_position() {
        let shared = make_shared(44100, 44100);
        shared.position.store(22050, Ordering::Relaxed);
        assert!((shared.position_secs() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn seek_to_frame_clamps() {
        let shared = make_shared(1000, 44100);
        shared.seek_to_frame(5000);
        assert_eq!(shared.seek_target.load(Ordering::Relaxed), 1000);
    }

    #[test]
    fn seek_to_secs() {
        let shared = make_shared(44100, 44100);
        shared.seek_to_secs(0.5);
        assert_eq!(shared.seek_target.load(Ordering::Relaxed), 22050);
    }
}

pub mod ffmpeg;
pub mod readback;

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::recording::ffmpeg::{EncodingPreset, FfmpegConfig, FfmpegPipe, PipeWriterThread};
use crate::recording::readback::DoubleReadbackBuffer;
use crate::types::Resolution;

/// Configuration for starting a recording session.
pub struct RecordingConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub output_path: PathBuf,
    pub audio_path: Option<PathBuf>,
    pub preset: EncodingPreset,
    pub custom_args: Option<String>,
    pub pre_roll_frames: u32,
    pub total_frames: u64,
}

/// Active recording session state.
pub struct RecordingState {
    pub pipe_writer: PipeWriterThread,
    pub readback: DoubleReadbackBuffer,
    pub offscreen_texture: wgpu::Texture,
    pub offscreen_view: wgpu::TextureView,
    pub resolution: Resolution,
    pub fps: u32,
    pub dt: f32,
    pub current_frame: u64,
    pub total_frames: u64,
    pub pre_roll_remaining: u32,
    pub started_at: Instant,
    pub samples_per_frame: usize,
}

impl RecordingState {
    /// Create a new recording session: allocates the offscreen texture,
    /// readback buffer, and spawns the ffmpeg pipe.
    pub fn start(
        device: &wgpu::Device,
        config: RecordingConfig,
        sample_rate: u32,
    ) -> io::Result<Self> {
        let width = config.width;
        let height = config.height;
        let fps = config.fps;

        let offscreen_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("recording_offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let offscreen_view = offscreen_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let readback = DoubleReadbackBuffer::new(device, width, height);

        let custom_args = config
            .custom_args
            .map(|s| s.split_whitespace().map(String::from).collect::<Vec<_>>());

        let ffmpeg_config = FfmpegConfig {
            width,
            height,
            fps: fps as f64,
            output_path: config.output_path,
            audio_path: config.audio_path,
            preset: config.preset,
            custom_args,
        };

        let pipe = FfmpegPipe::spawn(&ffmpeg_config).map_err(io::Error::other)?;
        let pipe_writer = PipeWriterThread::spawn(pipe, height);

        let resolution = Resolution::new(width, height);
        let dt = 1.0 / fps as f32;
        let samples_per_frame = (sample_rate as f32 / fps as f32).round() as usize;

        Ok(Self {
            pipe_writer,
            readback,
            offscreen_texture,
            offscreen_view,
            resolution,
            fps,
            dt,
            current_frame: 0,
            total_frames: config.total_frames,
            pre_roll_remaining: config.pre_roll_frames,
            started_at: Instant::now(),
            samples_per_frame,
        })
    }

    /// Returns true if the recording is still in the pre-roll phase.
    pub fn is_pre_roll(&self) -> bool {
        self.pre_roll_remaining > 0
    }

    /// Returns true if all frames have been encoded.
    pub fn is_complete(&self) -> bool {
        self.current_frame >= self.total_frames
    }

    /// Returns progress in the range [0, 1].
    pub fn progress(&self) -> f32 {
        if self.total_frames == 0 {
            return 1.0;
        }
        (self.current_frame as f32 / self.total_frames as f32).min(1.0)
    }

    /// Returns the elapsed time since the recording started.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Advance the frame counter: decrements pre-roll if still pre-rolling,
    /// otherwise increments the recorded frame count.
    /// Returns true on the frame where pre-roll just completed (transition).
    pub fn advance_frame(&mut self) -> bool {
        let was_pre_rolling = self.pre_roll_remaining > 0;
        if self.pre_roll_remaining > 0 {
            self.pre_roll_remaining -= 1;
        } else {
            self.current_frame += 1;
        }
        // Return true when we just transitioned out of pre-roll
        was_pre_rolling && self.pre_roll_remaining == 0
    }

    /// Flush the last pending frame and shut down the pipe writer thread.
    pub fn finish(self, device: &wgpu::Device) -> io::Result<()> {
        // The double-buffer is one frame behind — flush the final frame.
        if let Some(data) = self.readback.flush(device) {
            self.pipe_writer.send(data).map_err(io::Error::other)?;
        }
        self.pipe_writer.finish().map_err(io::Error::other)
    }
}

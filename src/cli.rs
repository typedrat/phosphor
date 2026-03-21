use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};

use crate::audio_output::SharedAudioPlayback;
use crate::gpu::GpuState;
use crate::phosphor::phosphor_database;
use crate::recording::ffmpeg::{EncodingPreset, FfmpegConfig, FfmpegPipe, PipeWriterThread};
use crate::recording::readback::DoubleReadbackBuffer;
use crate::simulation::InputState;
use crate::types::{InputMode, Resolution};

#[derive(Parser)]
#[command(name = "phosphor", about = "Physically-based X-Y CRT simulator")]
pub struct Cli {
    /// Record video output (headless mode, audio input only).
    /// Value is the output file path.
    #[arg(long)]
    pub record: Option<PathBuf>,

    /// Audio input file (required with --record)
    #[arg(long)]
    pub audio: Option<PathBuf>,

    /// Output resolution (WIDTHxHEIGHT)
    #[arg(long, default_value = "1920x1080")]
    pub resolution: String,

    /// Frames per second
    #[arg(long, default_value_t = 60)]
    pub fps: u32,

    /// Phosphor type (e.g. P1, P7, P31)
    #[arg(long, default_value = "P1")]
    pub phosphor: String,

    /// Encoding preset: h265-hdr10, h265-sdr, av1-hdr10, prores-4444
    #[arg(long, default_value = "h265-hdr10")]
    pub preset: String,

    /// Custom ffmpeg arguments (overrides preset)
    #[arg(long)]
    pub ffmpeg_args: Option<String>,

    /// Beam intensity (0.0 - 5.0)
    #[arg(long, default_value_t = 0.5)]
    pub intensity: f32,

    /// Beam focus / spot size (0.1 - 10.0)
    #[arg(long, default_value_t = 0.5)]
    pub focus: f32,

    /// Pre-roll frames (warm up phosphor before recording)
    #[arg(long)]
    pub pre_roll: Option<u32>,

    /// Pipe buffer size (frames buffered between render and ffmpeg).
    /// Use -1 for unbounded. Default: 2.
    #[arg(long, default_value_t = 2, allow_hyphen_values = true)]
    pub pipe_buffer: i32,
}

fn parse_resolution(s: &str) -> anyhow::Result<(u32, u32)> {
    let parts: Vec<&str> = s.split('x').collect();
    if parts.len() != 2 {
        anyhow::bail!("resolution must be WIDTHxHEIGHT (e.g. 1920x1080)");
    }
    let w: u32 = parts[0].parse()?;
    let h: u32 = parts[1].parse()?;
    if w == 0 || h == 0 {
        anyhow::bail!("resolution dimensions must be > 0");
    }
    Ok((w, h))
}

fn parse_preset(s: &str) -> anyhow::Result<EncodingPreset> {
    EncodingPreset::from_slug(s).ok_or_else(|| {
        let slugs: Vec<&str> = EncodingPreset::ALL.iter().map(|p| p.slug()).collect();
        anyhow::anyhow!(
            "unknown preset '{}'; valid options: {}",
            s,
            slugs.join(", ")
        )
    })
}

pub fn run_headless(cli: &Cli) -> anyhow::Result<()> {
    // Validate arguments
    let audio_path = cli
        .audio
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("--audio is required with --record"))?;
    let output_path = cli
        .record
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("--record output path is required"))?;

    // Check ffmpeg
    crate::recording::ffmpeg::check_ffmpeg()
        .map_err(|e| anyhow::anyhow!("ffmpeg check failed: {e}"))?;

    let (width, height) = parse_resolution(&cli.resolution)?;
    let preset = parse_preset(&cli.preset)?;
    let resolution = Resolution::new(width, height);

    // Decode audio
    eprintln!("Decoding audio: {}", audio_path.display());
    let decoded = crate::beam::audio::decode_audio_file(audio_path)?;
    let sample_rate = decoded.sample_rate;
    let channels = decoded.channels as usize;
    let total_audio_frames = decoded.samples.len() / channels;
    let duration_secs = total_audio_frames as f64 / sample_rate as f64;
    let total_frames = (duration_secs * cli.fps as f64).ceil() as u64;
    let samples_per_frame = (sample_rate as f32 / cli.fps as f32).round() as usize;
    let pre_roll_frames = cli.pre_roll.unwrap_or(cli.fps); // default 1 second

    eprintln!(
        "Audio: {} Hz, {:.1}s, {} frames at {} fps",
        sample_rate, duration_secs, total_frames, cli.fps,
    );

    // Create shared audio playback state (no cpal, just data)
    let shared = Arc::new(SharedAudioPlayback::new(decoded));
    shared.playing.store(true, Ordering::Relaxed);

    // Find phosphor
    let db = phosphor_database();
    let phosphor = db
        .iter()
        .find(|p| p.designation.eq_ignore_ascii_case(&cli.phosphor))
        .ok_or_else(|| {
            let names: Vec<&str> = db.iter().map(|p| p.designation.as_str()).collect();
            anyhow::anyhow!(
                "unknown phosphor '{}'; available: {}",
                cli.phosphor,
                names.join(", ")
            )
        })?;

    eprintln!(
        "Phosphor: {} ({})",
        phosphor.designation, phosphor.description
    );

    // Create wgpu instance/adapter/device (headless — no surface)
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..Default::default()
    });

    // enumerate_adapters works without a surface on Vulkan (request_adapter
    // with compatible_surface: None fails on some Linux Vulkan drivers).
    let adapter = instance
        .enumerate_adapters(wgpu::Backends::PRIMARY)
        .into_iter()
        .find(|a| a.get_info().device_type == wgpu::DeviceType::DiscreteGpu)
        .or_else(|| {
            instance
                .enumerate_adapters(wgpu::Backends::PRIMARY)
                .into_iter()
                .next()
        })
        .ok_or_else(|| anyhow::anyhow!("no suitable GPU adapter found"))?;

    eprintln!("GPU adapter: {}", adapter.get_info().name);

    let features = wgpu::Features::FLOAT32_FILTERABLE;
    let adapter_limits = adapter.limits();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("phosphor-headless"),
        required_features: features,
        required_limits: wgpu::Limits {
            max_storage_buffer_binding_size:
                adapter_limits.max_storage_buffer_binding_size.max(1 << 30),
            max_buffer_size: adapter_limits.max_buffer_size.max(1 << 30),
            ..wgpu::Limits::default()
        },
        ..Default::default()
    }))
    .map_err(|e| anyhow::anyhow!("failed to create GPU device: {e}"))?;

    // Create headless GpuState
    let mut gpu = GpuState::new_headless(instance, adapter, device, queue, resolution);
    gpu.switch_phosphor(phosphor);

    // Set beam params
    gpu.beam_params.sigma_core = cli.focus;
    gpu.beam_params.sigma_halo = 6.0;
    gpu.beam_params.halo_fraction = 0.03;
    gpu.composite_params.exposure = cli.intensity;
    gpu.composite_params.viewport_offset = [0.0, 0.0];
    gpu.composite_params.viewport_size = [width as f32, height as f32];

    // Create offscreen texture and readback buffer
    let offscreen_texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("headless_offscreen"),
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
    let mut readback = DoubleReadbackBuffer::new(&gpu.device, width, height);

    // Create InputState for audio generation
    let mut input = InputState::default();
    input.mode = InputMode::Audio;
    input.audio.shared = Some(Arc::clone(&shared));

    let aspect = width as f32 / height as f32;
    let dt = 1.0 / cli.fps as f32;

    // Install SIGINT handler
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let cancelled = Arc::clone(&cancelled);
        ctrlc::set_handler(move || {
            cancelled.store(true, Ordering::Relaxed);
            eprintln!("\nInterrupted — finishing current frame...");
        })
        .ok();
    }

    // Pre-roll: run frames without recording to warm up phosphor decay
    eprintln!("Pre-rolling {} frames...", pre_roll_frames);
    for _ in 0..pre_roll_frames {
        if cancelled.load(Ordering::Relaxed) {
            eprintln!("Cancelled during pre-roll");
            return Ok(());
        }

        let samples = input.generate_audio_samples_recording(
            cli.focus,
            aspect,
            width as f32,
            samples_per_frame,
        );
        gpu.render_offscreen(&samples, dt, &offscreen_view);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    }

    // Rewind audio for actual recording
    input.rewind_recording_audio();

    // Spawn ffmpeg pipe
    let custom_args = cli
        .ffmpeg_args
        .as_ref()
        .map(|s| s.split_whitespace().map(String::from).collect::<Vec<_>>());

    let ffmpeg_config = FfmpegConfig {
        width,
        height,
        fps: cli.fps as f64,
        output_path: output_path.clone(),
        audio_path: Some(audio_path.clone()),
        preset,
        custom_args,
    };

    let pipe = FfmpegPipe::spawn(&ffmpeg_config)
        .map_err(|e| anyhow::anyhow!("failed to spawn ffmpeg: {e}"))?;
    let ffmpeg_progress = Arc::clone(&pipe.progress);
    let pipe_writer = PipeWriterThread::spawn(pipe, height, cli.pipe_buffer);

    // Progress bar
    let pb = ProgressBar::new(total_frames);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.green/dim}] {pos}/{len} ({eta})\n  render {msg}",
        )
        .unwrap()
        .progress_chars("━╸─"),
    );
    pb.println(format!(
        "Recording: {}x{} @ {} fps, {} -> {}",
        width,
        height,
        cli.fps,
        audio_path.display(),
        output_path.display(),
    ));

    let started = Instant::now();
    let mut frame = 0u64;

    // Main render loop
    while frame < total_frames {
        if cancelled.load(Ordering::Relaxed) {
            pb.abandon_with_message("cancelled");
            break;
        }

        // Generate audio samples for this frame
        let samples = input.generate_audio_samples_recording(
            cli.focus,
            aspect,
            width as f32,
            samples_per_frame,
        );

        // Render offscreen
        gpu.render_offscreen(&samples, dt, &offscreen_view);

        // Readback
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("headless_readback"),
            });
        readback.copy_from_texture(&mut encoder, &offscreen_texture);
        gpu.queue.submit(std::iter::once(encoder.finish()));

        // Read back the PREVIOUS frame (double-buffered, one frame behind)
        if let Some(data) = readback.read_pending(&gpu.device)
            && let Err(e) = pipe_writer.send(data)
        {
            pb.abandon_with_message(format!("pipe error: {e}"));
            break;
        }

        readback.advance();

        frame += 1;
        pb.set_position(frame);

        // Update message with render + encode stats
        if frame.is_multiple_of(30) || frame == total_frames {
            let elapsed = started.elapsed().as_secs_f64();
            let render_fps = frame as f64 / elapsed;
            let enc = ffmpeg_progress.lock().unwrap().clone();
            if enc.encode_fps > 0.0 {
                pb.set_message(format!(
                    "{render_fps:.0} fps | encode {:.0} fps {:.0}x | {:.0} kbps | {}",
                    enc.encode_fps, enc.speed, enc.bitrate_kbps, enc.output_size,
                ));
            } else {
                pb.set_message(format!("{render_fps:.0} fps"));
            }
        }
    }

    // Flush the last pending frame (double-buffer is one frame behind)
    if let Some(data) = readback.flush(&gpu.device)
        && let Err(e) = pipe_writer.send(data)
    {
        pb.println(format!("Error sending final frame: {e}"));
    }

    // Finish ffmpeg — close channel and wait for the writer thread + ffmpeg
    pb.set_message("finalizing...");
    let elapsed = started.elapsed();
    if let Err(e) = pipe_writer.finish() {
        pb.println(format!("Warning: ffmpeg did not exit cleanly: {e}"));
    }

    pb.finish_with_message(format!(
        "done — {} frames in {:.1}s ({:.1} fps avg)",
        frame,
        elapsed.as_secs_f64(),
        frame as f64 / elapsed.as_secs_f64(),
    ));
    eprintln!("Output: {}", output_path.display());

    Ok(())
}

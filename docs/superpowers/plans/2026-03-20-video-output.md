# Video Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add offline video recording via ffmpeg pipe, with a Recording UI tab and headless CLI mode.

**Architecture:** Offline render loop driven by winit event loop (one frame per RedrawRequested), rendering to an offscreen Rgba16Float texture at arbitrary resolution, with GPU readback via staging buffer piped to an ffmpeg child process. Sim thread switches to batch-on-demand mode during recording. CLI mode bypasses winit entirely for headless audio-only rendering.

**Tech Stack:** wgpu 27, winit 0.30, egui 0.33, ffmpeg (CLI pipe), clap, ctrlc

**Spec:** `docs/superpowers/specs/2026-03-20-video-output-design.md`

---

## File Structure

| File                        | Responsibility                                                                               | Status    |
| --------------------------- | -------------------------------------------------------------------------------------------- | --------- |
| `src/recording/mod.rs`      | `RecordingState` struct, ffmpeg pipe management, frame readback, preset definitions          | New       |
| `src/recording/ffmpeg.rs`   | FFmpeg process spawning, availability check, preset → args conversion, audio codec selection | New       |
| `src/recording/readback.rs` | Staging buffer creation, texture-to-buffer copy, row-padding-aware pipe write                | New       |
| `src/gpu/preview_blit.rs`   | Preview blit pipeline (fullscreen triangle, bilinear sample)                                 | New       |
| `src/gpu/preview_blit.wgsl` | Texture-sample shader for preview blit                                                       | New       |
| `src/ui/recording_panel.rs` | Recording tab UI (output settings, encoding, progress, controls)                             | New       |
| `src/cli.rs`                | Clap argument parsing, headless render loop                                                  | New       |
| `src/gpu/mod.rs`            | Surface-optional init (`Option<Surface>`), `new_headless()`, offscreen render target         | Modify    |
| `src/gpu/composite.rs`      | No changes needed (already takes format param)                                               | Unchanged |
| `src/simulation.rs`         | Batch-on-demand mode via channel request/response, recording-specific audio generation       | Modify    |
| `src/app.rs`                | Recording state integration, frame loop mode switching                                       | Modify    |
| `src/ui/mod.rs`             | Recording tab enum variant, draw routing                                                     | Modify    |
| `src/frame.rs`              | State gating during recording                                                                | Modify    |
| `src/main.rs`               | CLI dispatch vs winit entry                                                                  | Modify    |
| `src/types.rs`              | Recording-related types if needed                                                            | Modify    |
| `Cargo.toml`                | Add `clap`, `ctrlc` dependencies                                                             | Modify    |
| `flake/packages.nix`        | Add `ffmpeg` to runtime wrapper                                                              | Modify    |

---

### Task 1: Add dependencies and Nix ffmpeg integration

**Files:**

- Modify: `Cargo.toml`
- Modify: `flake/packages.nix`

- [ ] **Step 1: Add clap and ctrlc to Cargo.toml**

Add to `[dependencies]` section:

```toml
clap = { version = "4", features = ["derive"] }
ctrlc = "3"
```

- [ ] **Step 2: Add ffmpeg to Nix package wrapper**

In `flake/packages.nix`, modify the `postInstall` in the `phosphor` package (line 54-57) to also add ffmpeg to PATH:

```nix
postInstall = ''
  wrapProgram $out/bin/phosphor \
    --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath runtimeLibs} \
    --prefix PATH : ${pkgs.lib.makeBinPath [pkgs.ffmpeg]}
'';
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check`
Expected: Compiles with new dependencies available.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock flake/packages.nix
git commit -m "chore: add clap, ctrlc deps and ffmpeg to Nix wrapper

Add clap for CLI argument parsing and ctrlc for SIGINT handling
in headless recording mode. Wire ffmpeg into the Nix package
wrapper PATH for video encoding."
```

---

### Task 2: FFmpeg pipe module

**Files:**

- Create: `src/recording/mod.rs`
- Create: `src/recording/ffmpeg.rs`

This task builds the ffmpeg process management in isolation — spawning, writing, closing, error detection. No GPU code yet.

- [ ] **Step 1: Create `src/recording/mod.rs`**

```rust
pub mod ffmpeg;
pub mod readback;
```

- [ ] **Step 2: Create `src/recording/ffmpeg.rs` with preset definitions**

```rust
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use tracing::{info, warn};

/// Encoding preset for video output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum EncodingPreset {
    #[default]
    H265Hdr10,
    H265Sdr,
    Av1Hdr10,
    ProRes4444,
}

impl EncodingPreset {
    pub const ALL: &[EncodingPreset] = &[
        EncodingPreset::H265Hdr10,
        EncodingPreset::H265Sdr,
        EncodingPreset::Av1Hdr10,
        EncodingPreset::ProRes4444,
    ];

    pub fn slug(&self) -> &'static str {
        match self {
            Self::H265Hdr10 => "h265-hdr10",
            Self::H265Sdr => "h265-sdr",
            Self::Av1Hdr10 => "av1-hdr10",
            Self::ProRes4444 => "prores-4444",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::H265Hdr10 => "H.265 HDR10",
            Self::H265Sdr => "H.265 SDR",
            Self::Av1Hdr10 => "AV1 HDR10",
            Self::ProRes4444 => "ProRes 4444",
        }
    }

    /// Recommended file extension for this preset.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::H265Hdr10 | Self::H265Sdr => "mp4",
            Self::Av1Hdr10 => "mkv",
            Self::ProRes4444 => "mov",
        }
    }

    /// FFmpeg video codec arguments for this preset.
    fn video_args(&self) -> Vec<&'static str> {
        match self {
            Self::H265Hdr10 => vec![
                "-c:v", "libx265",
                "-pix_fmt", "yuv420p10le",
                "-x265-params",
                "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc",
            ],
            Self::H265Sdr => vec![
                "-vf",
                "zscale=t=linear,tonemap=hable,zscale=t=bt709:p=bt709:m=bt709",
                "-c:v", "libx265",
                "-pix_fmt", "yuv420p",
            ],
            Self::Av1Hdr10 => vec![
                "-c:v", "libsvtav1",
                "-pix_fmt", "yuv420p10le",
                "-svtav1-params",
                "color-primaries=9:transfer-characteristics=16:matrix-coefficients=9",
            ],
            Self::ProRes4444 => vec![
                "-c:v", "prores_ks",
                "-profile:v", "4444",
                "-pix_fmt", "yuva444p10le",
            ],
        }
    }

    /// Parse a preset from its slug string.
    pub fn from_slug(s: &str) -> Option<Self> {
        match s {
            "h265-hdr10" => Some(Self::H265Hdr10),
            "h265-sdr" => Some(Self::H265Sdr),
            "av1-hdr10" => Some(Self::Av1Hdr10),
            "prores-4444" => Some(Self::ProRes4444),
            _ => None,
        }
    }
}

/// Check that ffmpeg is available and supports rgbaf16le pixel format.
pub fn check_ffmpeg() -> Result<(), String> {
    let output = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|_| "ffmpeg not found. Install ffmpeg or use the Nix package.".to_string())?;

    if !output.status.success() {
        return Err("ffmpeg found but returned an error.".to_string());
    }

    // Check rgbaf16le support
    let pix_output = Command::new("ffmpeg")
        .args(["-pix_fmts"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to query ffmpeg pixel formats: {e}"))?;

    let stdout = String::from_utf8_lossy(&pix_output.stdout);
    if !stdout.contains("rgbaf16le") {
        return Err(
            "ffmpeg does not support rgbaf16le pixel format. Install ffmpeg >= 5.0.".to_string(),
        );
    }

    Ok(())
}

/// Configuration for spawning an ffmpeg encoding process.
pub struct FfmpegConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub output_path: PathBuf,
    pub audio_path: Option<PathBuf>,
    pub preset: EncodingPreset,
    pub custom_args: Option<String>,
}

/// A running ffmpeg child process that accepts raw frame data on stdin.
pub struct FfmpegPipe {
    child: Child,
    bytes_per_row: usize,
    padded_bytes_per_row: usize,
}

impl FfmpegPipe {
    /// Spawn ffmpeg with the given configuration.
    pub fn spawn(config: &FfmpegConfig) -> io::Result<Self> {
        let mut args: Vec<String> = vec![
            "-y".into(),
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "rgbaf16le".into(),
            "-s".into(),
            format!("{}x{}", config.width, config.height),
            "-r".into(),
            config.fps.to_string(),
            "-i".into(),
            "pipe:0".into(),
        ];

        // Add audio input if present
        if let Some(audio_path) = &config.audio_path {
            args.extend(["-i".into(), audio_path.display().to_string()]);
            // Audio codec selection based on container compatibility
            let ext = config
                .output_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let audio_codec = match ext {
                "mov" => vec!["-c:a", "pcm_s16le"],
                _ => vec!["-c:a", "aac"],
            };
            args.extend(audio_codec.into_iter().map(String::from));
        }

        // Add video codec args (preset or custom)
        if let Some(custom) = &config.custom_args {
            args.extend(custom.split_whitespace().map(String::from));
        } else {
            args.extend(config.preset.video_args().into_iter().map(String::from));
        }

        args.push(config.output_path.display().to_string());

        info!("Spawning ffmpeg: ffmpeg {}", args.join(" "));

        let child = Command::new("ffmpeg")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;

        let bytes_per_row = config.width as usize * 8; // Rgba16Float = 8 bytes/pixel
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded_bytes_per_row = (bytes_per_row + alignment - 1) / alignment * alignment;

        Ok(Self {
            child,
            bytes_per_row,
            padded_bytes_per_row,
        })
    }

    /// Write one frame from a staging buffer (with row padding) to ffmpeg stdin.
    /// Returns Err if the pipe is broken (ffmpeg died).
    pub fn write_frame(&mut self, data: &[u8], height: u32) -> io::Result<()> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "ffmpeg stdin closed"))?;

        if self.bytes_per_row == self.padded_bytes_per_row {
            // No padding — write the whole buffer at once
            stdin.write_all(data)?;
        } else {
            // Strip row padding
            for row in 0..height as usize {
                let offset = row * self.padded_bytes_per_row;
                stdin.write_all(&data[offset..offset + self.bytes_per_row])?;
            }
        }

        Ok(())
    }

    /// Close the pipe and wait for ffmpeg to finish. Returns ffmpeg's exit status.
    pub fn finish(mut self) -> io::Result<std::process::ExitStatus> {
        // Drop stdin to signal EOF
        drop(self.child.stdin.take());
        let status = self.child.wait()?;
        if !status.success() {
            warn!("ffmpeg exited with status: {status}");
        } else {
            info!("ffmpeg finished successfully");
        }
        Ok(status)
    }
}

impl Drop for FfmpegPipe {
    fn drop(&mut self) {
        // Ensure stdin is closed and child is waited on
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}
```

- [ ] **Step 3: Register the recording module in main**

Add to `src/main.rs`:

```rust
mod recording;
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check`
Expected: Compiles. No tests yet — ffmpeg interaction is inherently integration-level.

- [ ] **Step 5: Commit**

```bash
git add src/recording/mod.rs src/recording/ffmpeg.rs src/main.rs
git commit -m "feat(recording): add ffmpeg pipe module with preset definitions

FfmpegPipe spawns ffmpeg as a child process, accepts raw Rgba16Float
frames with row-padding stripping, and handles graceful shutdown.
Includes encoding presets (h265-hdr10, h265-sdr, av1-hdr10, prores-4444)
and ffmpeg availability/capability checking."
```

---

### Task 3: GPU readback module

**Files:**

- Create: `src/recording/readback.rs`

Staging buffer creation and texture-to-buffer copy logic, isolated from the rest of the recording flow.

- [ ] **Step 1: Create `src/recording/readback.rs`**

```rust
use wgpu;

/// GPU staging buffer for reading back rendered frames to the CPU.
pub struct ReadbackBuffer {
    pub buffer: wgpu::Buffer,
    pub padded_bytes_per_row: u32,
    pub bytes_per_row: u32,
    pub width: u32,
    pub height: u32,
}

impl ReadbackBuffer {
    /// Create a new readback staging buffer for the given dimensions.
    /// Assumes Rgba16Float format (8 bytes per pixel).
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let bytes_per_row = width * 8; // Rgba16Float = 4 channels * 2 bytes
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = (bytes_per_row + alignment - 1) / alignment * alignment;
        let size = (padded_bytes_per_row * height) as u64;

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("recording_readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            buffer,
            padded_bytes_per_row,
            bytes_per_row,
            width,
            height,
        }
    }

    /// Encode a copy command from the offscreen texture to this staging buffer.
    pub fn copy_from_texture(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
    ) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Map the buffer, call the closure with the raw bytes, then unmap.
    /// Blocks until the GPU copy is complete.
    pub fn read_mapped<F>(&self, device: &wgpu::Device, f: F)
    where
        F: FnOnce(&[u8]),
    {
        let buffer_slice = self.buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });
        device.poll(wgpu::Maintain::Wait);
        receiver.recv().unwrap().expect("Failed to map readback buffer");

        let data = buffer_slice.get_mapped_range();
        f(&data);
        drop(data);
        self.buffer.unmap();
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add src/recording/readback.rs
git commit -m "feat(recording): add GPU readback staging buffer

ReadbackBuffer handles texture-to-buffer copies with proper row
alignment padding and synchronous mapped reads for piping to ffmpeg."
```

---

### Task 4: Preview blit pipeline

**Files:**

- Create: `src/gpu/preview_blit.wgsl`
- Create: `src/gpu/preview_blit.rs`

A minimal fullscreen-triangle pipeline that samples a texture with bilinear filtering, used to display the offscreen recording texture in the window.

- [ ] **Step 1: Create `src/gpu/preview_blit.wgsl`**

```wgsl
// Preview blit shader: samples an input texture and draws to screen.
// Uses the fullscreen triangle trick (vertex ID 0,1,2 → covers screen).

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var tex_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    // Fullscreen triangle: vertex 0 = (-1,-1), 1 = (3,-1), 2 = (-1,3)
    let x = f32(i32(vertex_index & 1u) * 4 - 1);
    let y = f32(i32(vertex_index >> 1u) * 4 - 1);
    out.position = vec4f(x, y, 0.0, 1.0);
    out.uv = vec2f((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    return textureSample(tex, tex_sampler, in.uv);
}
```

- [ ] **Step 2: Create `src/gpu/preview_blit.rs`**

```rust
use wgpu;

/// Pipeline for blitting an offscreen texture to the swapchain surface.
pub struct PreviewBlitPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl PreviewBlitPipeline {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview_blit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("preview_blit.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview_blit_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview_blit_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview_blit_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("preview_blit_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
        }
    }

    /// Blit the source texture to the target view (swapchain surface).
    pub fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
    ) {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview_blit_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("preview_blit_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
```

- [ ] **Step 3: Register the module**

In `src/gpu/mod.rs`, add after the other `pub mod` declarations:

```rust
pub mod preview_blit;
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check`
Expected: Compiles. The pipeline isn't wired in yet.

- [ ] **Step 5: Commit**

```bash
git add src/gpu/preview_blit.rs src/gpu/preview_blit.wgsl src/gpu/mod.rs
git commit -m "feat(gpu): add preview blit pipeline for recording preview

Fullscreen-triangle pipeline that samples an offscreen texture with
bilinear filtering and draws to the swapchain surface. Used to display
recording output in the window during offline rendering."
```

---

### Task 5: Recording state and offscreen render target

**Files:**

- Modify: `src/recording/mod.rs`
- Modify: `src/gpu/mod.rs`

Add `RecordingState` that owns the offscreen texture, readback buffer, ffmpeg pipe, and frame counter. Add methods on `GpuState` to create the offscreen texture and render to it.

- [ ] **Step 1: Expand `src/recording/mod.rs` with RecordingState**

```rust
pub mod ffmpeg;
pub mod readback;

use std::path::PathBuf;
use std::time::Instant;

use crate::recording::ffmpeg::{EncodingPreset, FfmpegConfig, FfmpegPipe};
use crate::recording::readback::ReadbackBuffer;
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
    pub pipe: FfmpegPipe,
    pub readback: ReadbackBuffer,
    pub offscreen_texture: wgpu::Texture,
    pub offscreen_view: wgpu::TextureView,
    pub resolution: Resolution,
    pub fps: u32,
    pub dt: f32,
    pub current_frame: u64,
    pub total_frames: u64,
    pub pre_roll_remaining: u32,
    pub started_at: Instant,
    /// Samples per video frame = sample_rate / fps
    pub samples_per_frame: usize,
}

impl RecordingState {
    /// Start a new recording session. Creates offscreen texture, staging buffer, and ffmpeg pipe.
    pub fn start(
        device: &wgpu::Device,
        config: RecordingConfig,
        sample_rate: f32,
    ) -> std::io::Result<Self> {
        let resolution = Resolution::new(config.width, config.height);

        // Create offscreen render target
        let offscreen_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("recording_offscreen"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING, // needed for preview blit sampling
            view_formats: &[],
        });
        let offscreen_view = offscreen_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create readback staging buffer
        let readback = ReadbackBuffer::new(device, config.width, config.height);

        // Spawn ffmpeg
        let ffmpeg_config = FfmpegConfig {
            width: config.width,
            height: config.height,
            fps: config.fps,
            output_path: config.output_path,
            audio_path: config.audio_path,
            preset: config.preset,
            custom_args: config.custom_args,
        };
        let pipe = FfmpegPipe::spawn(&ffmpeg_config)?;

        let dt = 1.0 / config.fps as f32;
        let samples_per_frame = (sample_rate / config.fps as f32).round() as usize;

        Ok(Self {
            pipe,
            readback,
            offscreen_texture,
            offscreen_view,
            resolution,
            fps: config.fps,
            dt,
            current_frame: 0,
            total_frames: config.total_frames,
            pre_roll_remaining: config.pre_roll_frames,
            started_at: Instant::now(),
            samples_per_frame,
        })
    }

    /// Whether we're still in the pre-roll warm-up phase.
    pub fn is_pre_roll(&self) -> bool {
        self.pre_roll_remaining > 0
    }

    /// Whether all frames have been rendered.
    pub fn is_complete(&self) -> bool {
        self.current_frame >= self.total_frames
    }

    /// Progress as a fraction [0, 1].
    pub fn progress(&self) -> f32 {
        if self.total_frames == 0 {
            1.0
        } else {
            self.current_frame as f32 / self.total_frames as f32
        }
    }

    /// Elapsed wall time since recording started.
    pub fn elapsed(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    /// Advance frame counter. If pre-rolling, decrements pre-roll counter instead.
    pub fn advance_frame(&mut self) {
        if self.pre_roll_remaining > 0 {
            self.pre_roll_remaining -= 1;
        } else {
            self.current_frame += 1;
        }
    }

    /// Finalize the recording — close ffmpeg pipe and return exit status.
    pub fn finish(self) -> std::io::Result<std::process::ExitStatus> {
        self.pipe.finish()
    }
}
```

- [ ] **Step 2: Add offscreen rendering support to `GpuState`**

In `src/gpu/mod.rs`, add `PreviewBlitPipeline` to `GpuState` struct and imports. Add it as an `Option<PreviewBlitPipeline>` field since it's only needed during recording:

After the existing imports at the top, add:

```rust
use crate::gpu::preview_blit::PreviewBlitPipeline;
```

Add field to `GpuState` struct (after `hdr_output: bool`):

```rust
pub preview_blit: Option<PreviewBlitPipeline>,
```

Initialize in `GpuState::new()` at the end of field initialization:

```rust
preview_blit: None,
```

Add method to `GpuState` to prepare for recording (creates preview blit pipeline and resizes buffers):

```rust
/// Prepare GPU state for recording at the given resolution.
/// Resizes internal buffers and creates the preview blit pipeline.
pub fn prepare_recording(&mut self, resolution: Resolution) {
    // Resize accumulation buffer, HDR buffer, and faceplate scatter textures
    self.resize_buffers(resolution);

    // Recreate composite pipeline targeting Rgba16Float
    self.composite = CompositePipeline::new(&self.device, wgpu::TextureFormat::Rgba16Float);

    // Create preview blit pipeline for displaying offscreen texture in window
    if let Some(ref surface_config) = self.surface_config_opt() {
        self.preview_blit = Some(PreviewBlitPipeline::new(&self.device, surface_config.format));
    }
}

/// Restore GPU state after recording ends.
pub fn end_recording(&mut self) {
    // Recreate composite pipeline targeting swapchain format
    if let Some(ref surface_config) = self.surface_config_opt() {
        let format = surface_config.format;
        self.composite = CompositePipeline::new(&self.device, format);

        // Resize buffers back to window resolution
        let resolution = Resolution::new(surface_config.width, surface_config.height);
        self.resize_buffers(resolution);
    }

    self.preview_blit = None;
}
```

Note: `surface_config_opt()` doesn't exist yet — for now, use `&self.surface_config` directly. The surface-optional refactor happens in Task 8 (CLI headless). For this task, just reference `self.surface_config` directly:

```rust
pub fn prepare_recording(&mut self, resolution: Resolution) {
    self.resize_buffers(resolution);
    self.composite = CompositePipeline::new(&self.device, wgpu::TextureFormat::Rgba16Float);
    self.preview_blit = Some(PreviewBlitPipeline::new(&self.device, self.surface_config.format));
}

pub fn end_recording(&mut self) {
    let format = self.surface_config.format;
    self.composite = CompositePipeline::new(&self.device, format);
    let resolution = Resolution::new(self.surface_config.width, self.surface_config.height);
    self.resize_buffers(resolution);
    self.preview_blit = None;
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add src/recording/mod.rs src/gpu/mod.rs
git commit -m "feat(recording): add RecordingState and GPU recording preparation

RecordingState owns the offscreen texture, readback buffer, ffmpeg pipe,
and frame counter. GpuState gains prepare_recording() and end_recording()
methods to resize buffers and recreate the composite pipeline for the
offscreen Rgba16Float target."
```

---

### Task 6: Sim thread batch-on-demand mode

**Files:**

- Modify: `src/simulation.rs`

Add a batch-on-demand mode to the sim thread for recording. When active, the sim thread waits for a frame request, generates exactly `samples_per_frame` samples, and sends them back.

**Critical: Audio batch generation.** The existing `InputMode::Audio` path in `generate_samples_fixed()` reads from `SharedAudioPlayback::position`, which is advanced by the cpal audio output callback. In recording mode, cpal isn't running, so the position never advances. The fix: add a `recording_audio_position` field to `AudioState` that the batch mode logic advances directly. In batch mode, the audio sample generation reads sequentially from the decoded buffer at `samples_per_frame` per request, ignoring `shared.position` entirely.

- [ ] **Step 1: Add recording audio position to AudioState**

In `src/simulation.rs`, add to `AudioState`:

```rust
/// Position cursor used during recording (bypasses cpal-driven SharedAudioPlayback::position).
pub recording_audio_pos: usize,
```

Initialize to 0 in `AudioState::default()`.

- [ ] **Step 2: Add a recording-specific audio generation method to InputState**

```rust
/// Generate audio beam samples for recording mode. Reads sequentially from
/// the decoded audio buffer, advancing an internal position cursor.
/// Unlike the real-time path, this does not depend on cpal playback position.
pub fn generate_audio_samples_recording(
    &mut self,
    focus: f32,
    aspect: f32,
    viewport_width: f32,
    count: usize,
) -> Vec<BeamSample> {
    let spot_radius = focus / viewport_width.max(1.0);
    let beam = BeamState { spot_radius };

    let Some(shared) = &self.audio.shared else {
        return Vec::new();
    };

    let channels = shared.channels as usize;
    let dt = 1.0 / shared.sample_rate as f32;
    let pos = self.audio.recording_audio_pos;
    let samples_data = &shared.samples;

    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        let frame = pos + i;
        let idx = frame * channels;
        if idx + 1 >= samples_data.len() {
            break;
        }
        let l = samples_data[idx];
        let r = samples_data[idx + 1];
        result.push(BeamSample {
            x: (l + 1.0) / 2.0,
            y: (r + 1.0) / 2.0,
            intensity: 1.0,
            dt,
        });
    }

    self.audio.recording_audio_pos = pos + result.len();

    // Apply aspect ratio correction and arc-length resampling
    // (same as generate_samples_fixed)
    if aspect > 1.0 {
        for s in &mut result { s.x = 0.5 + (s.x - 0.5) / aspect; }
    } else if aspect < 1.0 {
        for s in &mut result { s.y = 0.5 + (s.y - 0.5) * aspect; }
    }

    let mut result = crate::beam::resample::arc_length_resample(&result, spot_radius * 0.5);
    for s in &mut result { s.intensity *= BEAM_ENERGY_SCALE; }

    result
}

/// Reset recording audio position to 0 (for pre-roll rewind).
pub fn rewind_recording_audio(&mut self) {
    self.audio.recording_audio_pos = 0;
}
```

- [ ] **Step 3: Add batch-on-demand command and channel types to `src/simulation.rs`**

Add new variants to `SimCommand`:

```rust
/// Enter batch-on-demand mode for recording. The sim thread will wait for
/// frame requests on the provided receiver and send sample batches back.
StartBatchMode {
    samples_per_frame: usize,
    frame_request_rx: crossbeam_channel::Receiver<()>,
    frame_response_tx: crossbeam_channel::Sender<Vec<BeamSample>>,
},
/// Exit batch-on-demand mode and resume real-time generation.
StopBatchMode,
```

- [ ] **Step 4: Modify the sim thread loop to handle batch mode**

In `run_simulation()`, add a batch mode state variable before the main loop:

```rust
let mut batch_mode: Option<BatchModeState> = None;

struct BatchModeState {
    samples_per_frame: usize,
    frame_request_rx: crossbeam_channel::Receiver<()>,
    frame_response_tx: crossbeam_channel::Sender<Vec<BeamSample>>,
}
```

In the command processing section, handle the new variants:

```rust
SimCommand::StartBatchMode {
    samples_per_frame,
    frame_request_rx,
    frame_response_tx,
} => {
    batch_mode = Some(BatchModeState {
        samples_per_frame,
        frame_request_rx,
        frame_response_tx,
    });
    tracing::info!("Sim thread entering batch-on-demand mode ({samples_per_frame} samples/frame)");
}
SimCommand::StopBatchMode => {
    batch_mode = None;
    tracing::info!("Sim thread exiting batch-on-demand mode");
}
```

Replace the sample generation section with a mode check. In batch mode, use the recording-specific audio method for audio input, and the existing `generate_samples_fixed` for other modes:

```rust
if let Some(ref batch) = batch_mode {
    // Batch-on-demand: wait for frame request, generate, respond
    match batch.frame_request_rx.recv() {
        Ok(()) => {
            let samples = if input.mode == InputMode::Audio {
                input.generate_audio_samples_recording(
                    focus, aspect, viewport_width, batch.samples_per_frame,
                )
            } else {
                input.generate_samples_fixed(
                    focus, aspect, viewport_width, sample_rate, batch.samples_per_frame,
                )
            };
            let _ = batch.frame_response_tx.send(samples);
        }
        Err(_) => {
            // Channel closed — recording ended, exit batch mode
            batch_mode = None;
        }
    }
} else {
    // ... existing real-time generation code ...
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add src/simulation.rs
git commit -m "feat(simulation): add batch-on-demand mode for recording

Sim thread can enter batch mode via StartBatchMode command, where it
waits for per-frame requests and responds with exactly samples_per_frame
samples. This guarantees deterministic frame timing for offline recording."
```

---

### Task 7: Recording tab UI

**Files:**

- Create: `src/ui/recording_panel.rs`
- Modify: `src/ui/mod.rs`

Build the Recording tab with output settings, encoding settings, and controls.

- [ ] **Step 1: Add Recording variant to PanelTab**

In `src/ui/mod.rs` (line 20-25), add:

```rust
#[derive(Default, PartialEq)]
pub enum PanelTab {
    #[default]
    Scope,
    Engineer,
    Recording,
}
```

- [ ] **Step 2: Add recording UI state to `UiState`**

In `src/ui/mod.rs`, add the recording UI state fields to `UiState` struct:

```rust
pub recording: RecordingUiState,
```

And define `RecordingUiState`:

```rust
/// UI state for the Recording tab.
pub struct RecordingUiState {
    pub resolution_preset: ResolutionPreset,
    pub custom_width: u32,
    pub custom_height: u32,
    pub fps_preset: FpsPreset,
    pub custom_fps: u32,
    pub duration_secs: f32,
    pub output_path: Option<std::path::PathBuf>,
    pub encoding_preset: crate::recording::ffmpeg::EncodingPreset,
    pub custom_ffmpeg_args: String,
    /// None = not recording. Some = active recording progress.
    pub recording_progress: Option<RecordingProgress>,
}

pub struct RecordingProgress {
    pub current_frame: u64,
    pub total_frames: u64,
    pub elapsed: std::time::Duration,
    pub pre_rolling: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResolutionPreset {
    R720p,
    R1080p,
    R1440p,
    R4k,
    R8k,
    Custom,
}

impl ResolutionPreset {
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        match self {
            Self::R720p => Some((1280, 720)),
            Self::R1080p => Some((1920, 1080)),
            Self::R1440p => Some((2560, 1440)),
            Self::R4k => Some((3840, 2160)),
            Self::R8k => Some((7680, 4320)),
            Self::Custom => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::R720p => "720p",
            Self::R1080p => "1080p",
            Self::R1440p => "1440p",
            Self::R4k => "4K",
            Self::R8k => "8K",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FpsPreset {
    F24,
    F30,
    F60,
    F120,
    Custom,
}

impl FpsPreset {
    pub fn value(&self) -> Option<u32> {
        match self {
            Self::F24 => Some(24),
            Self::F30 => Some(30),
            Self::F60 => Some(60),
            Self::F120 => Some(120),
            Self::Custom => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::F24 => "24",
            Self::F30 => "30",
            Self::F60 => "60",
            Self::F120 => "120",
            Self::Custom => "Custom",
        }
    }
}

impl Default for RecordingUiState {
    fn default() -> Self {
        Self {
            resolution_preset: ResolutionPreset::R1080p,
            custom_width: 1920,
            custom_height: 1080,
            fps_preset: FpsPreset::F60,
            custom_fps: 60,
            duration_secs: 10.0,
            output_path: None,
            encoding_preset: Default::default(),
            custom_ffmpeg_args: String::new(),
            recording_progress: None,
        }
    }
}

impl RecordingUiState {
    pub fn effective_resolution(&self, max_dim: u32) -> (u32, u32) {
        let (w, h) = self.resolution_preset.dimensions()
            .unwrap_or((self.custom_width, self.custom_height));
        (w.min(max_dim), h.min(max_dim))
    }

    pub fn effective_fps(&self) -> u32 {
        self.fps_preset.value().unwrap_or(self.custom_fps)
    }

    pub fn is_recording(&self) -> bool {
        self.recording_progress.is_some()
    }
}
```

- [ ] **Step 3: Wire Recording tab into `draw_panels()`**

In `src/ui/mod.rs`, in the `draw_panels()` method (line 230-268), add the Recording tab button alongside the existing Scope/Engineer buttons:

```rust
ui.selectable_value(&mut self.tab, PanelTab::Recording, "Recording");
```

And add the dispatch:

```rust
PanelTab::Recording => {
    recording_panel::recording_panel(ui, &mut self.recording, self.input_mode);
}
```

- [ ] **Step 4: Create `src/ui/recording_panel.rs`**

```rust
use egui::Ui;

use crate::recording::ffmpeg::EncodingPreset;
use crate::types::InputMode;
use crate::ui::{FpsPreset, RecordingUiState, ResolutionPreset};

/// Draw the Recording tab contents.
pub fn recording_panel(ui: &mut Ui, state: &mut RecordingUiState, input_mode: InputMode) {
    let is_recording = state.is_recording();

    // --- Output Settings ---
    ui.heading("Output");
    ui.add_space(4.0);

    // Resolution
    ui.horizontal(|ui| {
        ui.label("Resolution:");
        egui::ComboBox::from_id_salt("res_preset")
            .selected_text(state.resolution_preset.label())
            .show_ui(ui, |ui| {
                for preset in [
                    ResolutionPreset::R720p,
                    ResolutionPreset::R1080p,
                    ResolutionPreset::R1440p,
                    ResolutionPreset::R4k,
                    ResolutionPreset::R8k,
                    ResolutionPreset::Custom,
                ] {
                    ui.selectable_value(
                        &mut state.resolution_preset,
                        preset,
                        preset.label(),
                    );
                }
            });
    });

    if state.resolution_preset == ResolutionPreset::Custom {
        ui.horizontal(|ui| {
            ui.label("W:");
            ui.add(egui::DragValue::new(&mut state.custom_width).range(64..=8192));
            ui.label("H:");
            ui.add(egui::DragValue::new(&mut state.custom_height).range(64..=8192));
        });
    }

    // FPS
    ui.horizontal(|ui| {
        ui.label("FPS:");
        egui::ComboBox::from_id_salt("fps_preset")
            .selected_text(state.fps_preset.label())
            .show_ui(ui, |ui| {
                for preset in [FpsPreset::F24, FpsPreset::F30, FpsPreset::F60, FpsPreset::F120, FpsPreset::Custom] {
                    ui.selectable_value(&mut state.fps_preset, preset, preset.label());
                }
            });
    });

    if state.fps_preset == FpsPreset::Custom {
        ui.horizontal(|ui| {
            ui.label("Custom FPS:");
            ui.add(egui::DragValue::new(&mut state.custom_fps).range(1..=240));
        });
    }

    // Duration (only for oscilloscope/vector modes)
    if input_mode != InputMode::Audio {
        ui.horizontal(|ui| {
            ui.label("Duration (s):");
            ui.add(
                egui::DragValue::new(&mut state.duration_secs)
                    .range(0.1..=3600.0)
                    .speed(0.5),
            );
        });
    }

    // Output path
    ui.horizontal(|ui| {
        ui.label("Output:");
        if let Some(ref path) = state.output_path {
            ui.label(path.file_name().unwrap_or_default().to_string_lossy().to_string());
        } else {
            ui.label("(none)");
        }
        if ui.button("Browse...").clicked() && !is_recording {
            let ext = state.encoding_preset.extension();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Video", &[ext])
                .save_file()
            {
                state.output_path = Some(path);
            }
        }
    });

    ui.add_space(8.0);

    // --- Encoding Settings ---
    ui.heading("Encoding");
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("Preset:");
        egui::ComboBox::from_id_salt("encoding_preset")
            .selected_text(state.encoding_preset.description())
            .show_ui(ui, |ui| {
                for preset in EncodingPreset::ALL {
                    if ui
                        .selectable_label(
                            state.encoding_preset == *preset,
                            preset.description(),
                        )
                        .clicked()
                    {
                        state.encoding_preset = preset.clone();
                    }
                }
            });
    });

    ui.horizontal(|ui| {
        ui.label("Custom args:");
        ui.text_edit_singleline(&mut state.custom_ffmpeg_args);
    });

    ui.add_space(8.0);

    // --- Controls ---
    if let Some(ref progress) = state.recording_progress {
        // Recording in progress
        let pct = if progress.total_frames > 0 {
            progress.current_frame as f32 / progress.total_frames as f32
        } else {
            0.0
        };

        if progress.pre_rolling {
            ui.label("Pre-rolling...");
        } else {
            ui.label(format!(
                "Frame {}/{} ({:.1}%)",
                progress.current_frame,
                progress.total_frames,
                pct * 100.0,
            ));
        }

        ui.add(egui::ProgressBar::new(pct));

        let elapsed = progress.elapsed;
        ui.label(format!(
            "Elapsed: {}:{:02}",
            elapsed.as_secs() / 60,
            elapsed.as_secs() % 60,
        ));

        // Cancel button - sets a flag that app.rs reads
        if ui.button("Cancel").clicked() {
            // Signal cancellation — app.rs will handle the actual teardown
            // by checking this field and calling recording.finish()
        }
    } else {
        // Not recording — show Record button
        let can_record = state.output_path.is_some();
        let record_btn = ui.add_enabled(can_record, egui::Button::new("Record"));
        if record_btn.clicked() {
            // Signal to start recording — app.rs handles the actual setup
            // by reading this state and creating RecordingState
        }
    }
}
```

- [ ] **Step 5: Add `recording_panel` module declaration**

In `src/ui/mod.rs`, add with the other module declarations:

```rust
pub mod recording_panel;
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check`
Expected: Compiles.

- [ ] **Step 7: Commit**

```bash
git add src/ui/recording_panel.rs src/ui/mod.rs
git commit -m "feat(ui): add Recording tab with output/encoding settings

Third tab in the side panel with resolution presets (720p-8K + custom),
FPS selection, duration for non-audio modes, output file picker,
encoding preset dropdown (h265-hdr10, h265-sdr, av1-hdr10, prores-4444),
custom ffmpeg args, and record/cancel controls with progress bar."
```

---

### Task 8: Wire recording into the app frame loop

**Files:**

- Modify: `src/app.rs`
- Modify: `src/frame.rs`
- Modify: `src/gpu/mod.rs`

This is the integration task — connecting RecordingState to the frame loop, switching between real-time and recording modes, handling the offscreen render + readback + pipe cycle.

- [ ] **Step 1: Add recording state and batch channels to App struct**

In `src/app.rs`, add fields to `App`:

```rust
recording: Option<crate::recording::RecordingState>,
/// Channel to request a batch of samples from the sim thread during recording.
batch_request_tx: Option<crossbeam_channel::Sender<()>>,
/// Channel to receive a batch of samples from the sim thread during recording.
batch_response_rx: Option<crossbeam_channel::Receiver<Vec<crate::beam::BeamSample>>>,
/// Whether the user clicked Cancel in the recording UI.
recording_cancel_requested: bool,
```

Initialize all as `None`/`false` in `App::new()` or wherever the struct is constructed.

- [ ] **Step 2: Add start_recording and stop_recording methods to App**

```rust
fn start_recording(&mut self) {
    let gpu = self.gpu.as_mut().unwrap();
    let ui = self.ui.as_ref().unwrap();
    let recording_ui = &ui.recording;

    // Check ffmpeg availability
    if let Err(msg) = crate::recording::ffmpeg::check_ffmpeg() {
        tracing::error!("Cannot start recording: {msg}");
        // TODO: surface error in UI
        return;
    }

    let (width, height) = recording_ui.effective_resolution(
        gpu.device.limits().max_texture_dimension_2d,
    );
    let fps = recording_ui.effective_fps();

    // Calculate total frames
    let total_frames = if ui.input_mode == crate::types::InputMode::Audio {
        // Audio duration from shared playback state
        // TODO: get audio duration from SharedAudioPlayback
        0u64 // placeholder
    } else {
        (recording_ui.duration_secs * fps as f32).ceil() as u64
    };

    let resolution = crate::types::Resolution::new(width, height);
    let pre_roll_frames = fps; // 1 second of pre-roll

    // Create batch-on-demand channels
    let (request_tx, request_rx) = crossbeam_channel::bounded(1);
    let (response_tx, response_rx) = crossbeam_channel::unbounded();

    let samples_per_frame = (self.sample_rate / fps as f32).round() as usize;

    // Tell sim thread to enter batch mode
    if let Some(ref tx) = self.sim_commands {
        let _ = tx.send(crate::simulation::SimCommand::StartBatchMode {
            samples_per_frame,
            frame_request_rx: request_rx,
            frame_response_tx: response_tx,
        });
    }

    // Prepare GPU for recording resolution
    gpu.prepare_recording(resolution);

    // Create recording state
    let config = crate::recording::RecordingConfig {
        width,
        height,
        fps,
        output_path: recording_ui.output_path.clone().unwrap(),
        audio_path: None, // TODO: wire audio path from AudioUiState
        preset: recording_ui.encoding_preset.clone(),
        custom_args: if recording_ui.custom_ffmpeg_args.is_empty() {
            None
        } else {
            Some(recording_ui.custom_ffmpeg_args.clone())
        },
        pre_roll_frames,
        total_frames,
    };

    match crate::recording::RecordingState::start(&gpu.device, config, self.sample_rate) {
        Ok(state) => {
            self.recording = Some(state);
            self.batch_request_tx = Some(request_tx);
            self.batch_response_rx = Some(response_rx);
            self.recording_cancel_requested = false;
            tracing::info!("Recording started: {width}x{height} @ {fps}fps, {total_frames} frames");
        }
        Err(e) => {
            tracing::error!("Failed to start recording: {e}");
            // Tell sim thread to exit batch mode
            if let Some(ref tx) = self.sim_commands {
                let _ = tx.send(crate::simulation::SimCommand::StopBatchMode);
            }
            gpu.end_recording();
        }
    }
}

fn stop_recording(&mut self) {
    if let Some(recording) = self.recording.take() {
        match recording.finish() {
            Ok(status) => tracing::info!("Recording finished: {status}"),
            Err(e) => tracing::error!("Recording finish error: {e}"),
        }
    }

    // Tell sim thread to exit batch mode
    if let Some(ref tx) = self.sim_commands {
        let _ = tx.send(crate::simulation::SimCommand::StopBatchMode);
    }

    // Drop batch channels
    self.batch_request_tx = None;
    self.batch_response_rx = None;
    self.recording_cancel_requested = false;

    // Restore GPU state
    if let Some(ref mut gpu) = self.gpu {
        gpu.end_recording();
    }

    // Clear recording progress in UI
    if let Some(ref mut ui) = self.ui {
        ui.recording.recording_progress = None;
    }
}
```

- [ ] **Step 3: Modify RedrawRequested handler for recording mode**

In the `RedrawRequested` handler in `app.rs`, add recording-specific logic. When recording is active:

1. Request a batch of samples from the sim thread (instead of draining the ring buffer)
2. Render to the offscreen texture
3. Readback and pipe to ffmpeg (unless pre-rolling)
4. Preview blit to swapchain
5. Advance frame counter
6. Check for completion or cancellation

The key modification is to the sample acquisition and render call:

```rust
// In RedrawRequested handler, replace sample draining with:
let (samples, sim_dt) = if let Some(ref recording) = self.recording {
    // Recording mode: request batch from sim thread
    if let (Some(ref tx), Some(ref rx)) = (&self.batch_request_tx, &self.batch_response_rx) {
        let _ = tx.send(());
        match rx.recv() {
            Ok(batch) => (batch, recording.dt),
            Err(_) => (vec![], recording.dt),
        }
    } else {
        (vec![], recording.dt)
    }
} else {
    // Normal mode: drain ring buffer
    let max_dt = self.frame_interval.as_secs_f32() * 8.0;
    let max_samples = (self.sample_rate * max_dt) as usize;
    let samples = self.sim_consumer.as_mut()
        .map(|c| c.drain_up_to(max_samples))
        .unwrap_or_default();
    let sim_dt = if samples.is_empty() {
        0.0
    } else {
        samples.len() as f32 / self.sample_rate
    };
    (samples, sim_dt)
};
```

After the render call, add recording frame handling:

```rust
// Recording: readback + pipe + advance
if let Some(ref mut recording) = self.recording {
    let gpu = self.gpu.as_ref().unwrap();

    // Copy offscreen texture to staging buffer
    // (This needs to be part of the command encoder — see note below)
    // For now, create a new encoder for the copy
    let mut copy_encoder = gpu.device.create_command_encoder(
        &wgpu::CommandEncoderDescriptor { label: Some("readback_copy") },
    );
    recording.readback.copy_from_texture(&mut copy_encoder, &recording.offscreen_texture);
    gpu.queue.submit(std::iter::once(copy_encoder.finish()));

    // Read back and pipe to ffmpeg (skip during pre-roll)
    if !recording.is_pre_roll() {
        recording.readback.read_mapped(&gpu.device, |data| {
            if let Err(e) = recording.pipe.write_frame(data, recording.resolution.height) {
                tracing::error!("FFmpeg pipe write failed: {e}");
                self.recording_cancel_requested = true;
            }
        });
    }

    recording.advance_frame();

    // Update UI progress
    if let Some(ref mut ui) = self.ui {
        ui.recording.recording_progress = Some(crate::ui::RecordingProgress {
            current_frame: recording.current_frame,
            total_frames: recording.total_frames,
            elapsed: recording.elapsed(),
            pre_rolling: recording.is_pre_roll(),
        });
    }

    // Check completion or cancellation
    if recording.is_complete() || self.recording_cancel_requested {
        // Can't call stop_recording() here due to borrow, set a flag
    }
}
```

Note: The pipe write inside `read_mapped` has a borrow conflict with `recording.pipe`. This needs restructuring — the `write_frame` call can't borrow `recording` mutably while `read_mapped` borrows `readback`. Solution: extract the data to a temporary buffer:

```rust
if !recording.is_pre_roll() {
    let mut frame_data = Vec::new();
    recording.readback.read_mapped(&gpu.device, |data| {
        frame_data.extend_from_slice(data);
    });
    if let Err(e) = recording.pipe.write_frame(&frame_data, recording.resolution.height) {
        tracing::error!("FFmpeg pipe write failed: {e}");
        self.recording_cancel_requested = true;
    }
}
```

- [ ] **Step 4: Modify about_to_wait() for recording mode**

In `about_to_wait()`, when recording, use `ControlFlow::Poll` instead of `WaitUntil`:

```rust
fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
    // ... existing redraw requests ...

    if self.recording.is_some() {
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    } else {
        // ... existing WaitUntil logic ...
    }
}
```

- [ ] **Step 5: Modify render() to support offscreen target**

In `src/gpu/mod.rs`, the `render()` method needs to accept an optional offscreen texture view to render the composite pass to. The simplest approach: add a parameter `offscreen_target: Option<&wgpu::TextureView>`. When present, composite renders to the offscreen target, then preview_blit renders from offscreen to the swapchain.

Modify `GpuState::render()` signature:

```rust
pub fn render(
    &mut self,
    samples: &[BeamSample],
    dt: f32,
    egui: Option<&EguiRenderOutput>,
    overlay: Option<(&mut egui_wgpu::Renderer, &EguiRenderOutput)>,
    offscreen_target: Option<&wgpu::TextureView>,
) -> Result<(), wgpu::SurfaceError>
```

In the composite pass call, use the offscreen target if provided:

```rust
let composite_target = offscreen_target.unwrap_or(&view);
self.composite.render(
    &self.device,
    &mut encoder,
    composite_target,
    &self.composite_params,
    &self.hdr,
    &self.faceplate_scatter_textures,
);
```

When offscreen rendering, add preview blit before egui:

```rust
if let (Some(offscreen), Some(ref blit)) = (offscreen_target, &self.preview_blit) {
    blit.render(&self.device, &mut encoder, offscreen, &view);
}
```

Update all existing call sites to pass `None` for `offscreen_target`. This includes the call in `src/controls_window.rs` if it calls `gpu.render()` for the detached CRT viewport.

- [ ] **Step 6: Add state gating in frame.rs**

In `src/frame.rs`, modify `sync_gpu_params()` to skip parameter updates when recording:

```rust
pub fn sync_gpu_params(gpu: &mut GpuState, ui: &UiState, is_recording: bool) {
    if is_recording {
        // During recording, only update the composite params that don't affect
        // pipeline state (exposure is already frozen). Skip all resizing.
        return;
    }
    // ... existing code ...
}
```

Similarly gate `dispatch_sim_commands()`:

```rust
pub fn dispatch_sim_commands(
    tx: &crossbeam_channel::Sender<SimCommand>,
    ui: &mut UiState,
    gpu: &GpuState,
    sidebar_width: f32,
    sample_rate: &mut f32,
    sim_consumer: &mut Option<SampleConsumer>,
    is_recording: bool,
) {
    if is_recording {
        return; // Don't send commands during recording
    }
    // ... existing code ...
}
```

- [ ] **Step 7: Verify it compiles and test manually**

Run: `cargo check`
Then: `cargo run`
Verify: App launches normally, Recording tab appears, basic UI is functional.

- [ ] **Step 8: Commit**

```bash
git add src/app.rs src/frame.rs src/gpu/mod.rs
git commit -m "feat: integrate recording into app frame loop

Wire RecordingState into the winit event loop. In recording mode:
- Sim thread batch-on-demand via crossbeam channels
- Composite renders to offscreen Rgba16Float texture
- Staging buffer readback pipes to ffmpeg
- Preview blit shows recording output in window
- ControlFlow::Poll for maximum render speed
- State gating freezes all Scope/Engineer controls"
```

---

### Task 9: End-to-end recording flow (start/stop/cancel)

**Files:**

- Modify: `src/app.rs`
- Modify: `src/ui/recording_panel.rs`
- Modify: `src/ui/mod.rs`

Wire up the Record/Cancel buttons to actually start and stop recording. Handle completion detection.

- [ ] **Step 1: Add recording action signals to RecordingUiState**

In `src/ui/mod.rs`, add action fields:

```rust
pub struct RecordingUiState {
    // ... existing fields ...
    /// Set by UI button, cleared by app after reading.
    pub start_requested: bool,
    /// Set by UI button, cleared by app after reading.
    pub cancel_requested: bool,
}
```

- [ ] **Step 2: Wire Record button in recording_panel.rs**

Update the Record button handler:

```rust
if record_btn.clicked() {
    state.start_requested = true;
}
```

Update the Cancel button handler:

```rust
if ui.button("Cancel").clicked() {
    state.cancel_requested = true;
}
```

- [ ] **Step 3: Process recording signals in RedrawRequested**

At the beginning of the `RedrawRequested` handler in `app.rs`, check for start/stop/cancel:

```rust
// Check recording start/stop signals
if let Some(ref mut ui) = self.ui {
    if ui.recording.start_requested {
        ui.recording.start_requested = false;
        self.start_recording();
    }
    if ui.recording.cancel_requested || self.recording_cancel_requested {
        ui.recording.cancel_requested = false;
        self.stop_recording();
    }
}

// Check recording completion
if self.recording.as_ref().map_or(false, |r| r.is_complete()) {
    self.stop_recording();
}
```

- [ ] **Step 4: Wire audio duration calculation**

In `start_recording()`, calculate audio duration from `SharedAudioPlayback`:

```rust
let total_frames = if ui.input_mode == crate::types::InputMode::Audio {
    // Get audio duration from the shared playback state
    if let Some(ref audio_output) = self.audio_output {
        if let Some(ref shared) = audio_output.shared() {
            let total_audio_samples = shared.samples.len() / shared.channels as usize;
            let audio_duration_secs = total_audio_samples as f64 / shared.sample_rate as f64;
            (audio_duration_secs * fps as f64).ceil() as u64
        } else {
            tracing::error!("No audio loaded for recording");
            return;
        }
    } else {
        tracing::error!("No audio output for recording");
        return;
    }
} else {
    (recording_ui.duration_secs * fps as f32).ceil() as u64
};
```

- [ ] **Step 5: Wire audio file path for ffmpeg muxing**

In `start_recording()`, set the audio path in the config:

```rust
let audio_path = if ui.input_mode == crate::types::InputMode::Audio {
    ui.audio_ui.file_path.clone() // The path of the loaded audio file
} else {
    None
};
```

- [ ] **Step 6: Verify the full recording flow**

Run: `cargo run`
Test: Set up oscilloscope mode, go to Recording tab, set output path, click Record. Verify frames render, progress bar updates, and the file is created. Cancel works.

- [ ] **Step 7: Commit**

```bash
git add src/app.rs src/ui/recording_panel.rs src/ui/mod.rs
git commit -m "feat: wire recording start/stop/cancel flow

Record button checks ffmpeg availability, creates RecordingState,
enters batch-on-demand mode. Cancel and completion trigger graceful
teardown. Audio duration and file path wired for audio mode muxing."
```

---

### Task 10: Pre-roll and audio seek

**Files:**

- Modify: `src/app.rs`
- Modify: `src/simulation.rs`

Implement pre-roll warm-up (render frames without piping) and audio position rewind after pre-roll.

- [ ] **Step 1: Add RewindRecordingAudio command to SimCommand**

In `src/simulation.rs`, add:

```rust
/// Rewind the recording audio position cursor to 0.
RewindRecordingAudio,
```

Handle in the command processing:

```rust
SimCommand::RewindRecordingAudio => {
    input.rewind_recording_audio();
}
```

- [ ] **Step 2: Track pre-roll transition in RecordingState**

In `src/recording/mod.rs`, add to `RecordingState`:

```rust
/// Set to true when pre-roll was active last frame, false otherwise.
/// Used to detect the pre-roll → recording transition.
was_pre_rolling: bool,
```

Initialize to `true` in `RecordingState::start()`.

Update `advance_frame()`:

```rust
pub fn advance_frame(&mut self) -> bool {
    let just_finished_preroll = self.was_pre_rolling && self.pre_roll_remaining == 0;
    self.was_pre_rolling = self.pre_roll_remaining > 0;

    if self.pre_roll_remaining > 0 {
        self.pre_roll_remaining -= 1;
    } else {
        self.current_frame += 1;
    }

    // Returns true on the frame where pre-roll just completed
    just_finished_preroll
}
```

- [ ] **Step 3: Rewind audio after pre-roll in app.rs**

In the recording frame loop, after `advance_frame()`:

```rust
let preroll_just_ended = recording.advance_frame();
if preroll_just_ended {
    if let Some(ref tx) = self.sim_commands {
        let _ = tx.send(crate::simulation::SimCommand::RewindRecordingAudio);
    }
    tracing::info!("Pre-roll complete, audio rewound to start");
}
```

- [ ] **Step 3: Verify pre-roll works**

Run: `cargo run`
Test: Start recording with a slow-decay phosphor (P7). The first visible frame should have accumulated glow, not start from black.

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/simulation.rs
git commit -m "feat(recording): implement pre-roll warm-up with audio rewind

Render configurable pre-roll frames (default: 1 second) before piping
to ffmpeg. For audio mode, rewind playback to t=0 after pre-roll so
the recorded video starts from the beginning of the audio track."
```

---

### Task 11: CLI headless mode

**Files:**

- Create: `src/cli.rs`
- Modify: `src/main.rs`
- Modify: `src/gpu/mod.rs`

Add `--record` CLI flag with clap that bypasses winit and runs a headless render loop.

- [ ] **Step 1: Create `src/cli.rs` with clap argument definitions**

```rust
use std::path::PathBuf;

use clap::Parser;

use crate::recording::ffmpeg::EncodingPreset;

#[derive(Parser)]
#[command(name = "phosphor", about = "Physically-based X-Y CRT simulator")]
pub struct Cli {
    /// Record video output (headless mode, audio input only)
    #[arg(long)]
    pub record: Option<PathBuf>,

    /// Audio input file (required with --record)
    #[arg(long)]
    pub audio: Option<PathBuf>,

    /// Output resolution (WxH)
    #[arg(long, default_value = "1920x1080")]
    pub resolution: String,

    /// Frame rate
    #[arg(long, default_value_t = 60)]
    pub fps: u32,

    /// Phosphor type
    #[arg(long, default_value = "P1")]
    pub phosphor: String,

    /// Encoding preset (h265-hdr10, h265-sdr, av1-hdr10, prores-4444)
    #[arg(long, default_value = "h265-hdr10")]
    pub preset: String,

    /// Custom ffmpeg arguments (overrides preset)
    #[arg(long)]
    pub ffmpeg_args: Option<String>,

    /// Beam intensity
    #[arg(long, default_value_t = 0.5)]
    pub intensity: f32,

    /// Beam focus
    #[arg(long, default_value_t = 0.5)]
    pub focus: f32,

    /// Number of pre-roll warm-up frames
    #[arg(long)]
    pub pre_roll: Option<u32>,
}

impl Cli {
    pub fn parse_resolution(&self) -> Result<(u32, u32), String> {
        let parts: Vec<&str> = self.resolution.split('x').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid resolution format: '{}'. Expected WxH.", self.resolution));
        }
        let width = parts[0].parse::<u32>().map_err(|_| "Invalid width")?;
        let height = parts[1].parse::<u32>().map_err(|_| "Invalid height")?;
        Ok((width, height))
    }

    pub fn parse_preset(&self) -> Result<EncodingPreset, String> {
        EncodingPreset::from_slug(&self.preset)
            .ok_or_else(|| format!("Unknown preset '{}'. Options: h265-hdr10, h265-sdr, av1-hdr10, prores-4444", self.preset))
    }
}

/// Run headless recording mode.
pub fn run_headless(cli: &Cli) -> anyhow::Result<()> {
    use crate::beam::audio::decode_audio_file;
    use crate::recording::{ffmpeg, RecordingConfig, RecordingState};
    use crate::simulation::InputState;
    use crate::types::Resolution;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let output_path = cli.record.as_ref().unwrap();
    let audio_path = cli.audio.as_ref()
        .ok_or_else(|| anyhow::anyhow!("--audio is required with --record"))?;

    // Check ffmpeg
    ffmpeg::check_ffmpeg().map_err(|e| anyhow::anyhow!(e))?;

    // Parse options
    let (width, height) = cli.parse_resolution().map_err(|e| anyhow::anyhow!(e))?;
    let preset = cli.parse_preset().map_err(|e| anyhow::anyhow!(e))?;
    let pre_roll = cli.pre_roll.unwrap_or(cli.fps);

    // Decode audio
    eprintln!("Decoding audio: {}", audio_path.display());
    let decoded = decode_audio_file(audio_path)?;
    let total_audio_samples = decoded.samples.len() / decoded.channels as usize;
    let audio_duration = total_audio_samples as f64 / decoded.sample_rate as f64;
    let total_frames = (audio_duration * cli.fps as f64).ceil() as u64;

    eprintln!("Audio: {:.1}s @ {}Hz, {} frames to render",
        audio_duration, decoded.sample_rate, total_frames);

    // Initialize GPU (headless — no surface)
    eprintln!("Initializing GPU...");
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok_or_else(|| anyhow::anyhow!("No suitable GPU adapter found"))?;

    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("headless"),
            required_features: wgpu::Features::FLOAT32_FILTERABLE,
            required_limits: wgpu::Limits {
                max_storage_buffer_binding_size: 1 << 30, // 1 GB
                max_buffer_size: 1 << 30,
                ..Default::default()
            },
            memory_hints: Default::default(),
        },
        None,
    ))?;

    let resolution = Resolution::new(width, height);

    // TODO: Initialize pipelines (beam_write, spectral_resolve, decay, scatter, composite)
    // at the recording resolution with the selected phosphor type.
    // This requires building GpuState without a surface — the new_headless() constructor.
    // For now, this is a skeleton that will be fleshed out when GpuState gets surface-optional init.

    eprintln!("Headless rendering not yet fully implemented — GpuState needs surface-optional init.");
    eprintln!("Use the UI recording mode for now.");

    // Install SIGINT handler
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_clone = cancelled.clone();
    ctrlc::set_handler(move || {
        cancelled_clone.store(true, Ordering::Relaxed);
    })?;

    // TODO: Main render loop (completed in Task 12)
    // Uses input.generate_audio_samples_recording() for deterministic
    // sequential audio sample generation (not cpal-driven).

    Ok(())
}
```

- [ ] **Step 2: Modify `src/main.rs` to dispatch CLI vs UI mode**

```rust
use clap::Parser;

mod cli;
// ... other mod declarations ...

fn main() {
    // Parse CLI args first
    let cli = cli::Cli::parse();

    // Initialize tracing
    // ... existing tracing setup ...

    if cli.record.is_some() {
        // Headless recording mode
        if let Err(e) = cli::run_headless(&cli) {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    } else {
        // Normal UI mode
        let event_loop = winit::event_loop::EventLoop::new().unwrap();
        let mut app = app::App::default();
        event_loop.run_app(&mut app).unwrap();
    }
}
```

- [ ] **Step 3: Make GpuState surface-optional (initial scaffolding)**

In `src/gpu/mod.rs`, change `surface` and `surface_config` to `Option`:

```rust
pub struct GpuState {
    // ... existing fields ...
    pub surface: Option<wgpu::Surface<'static>>,
    pub surface_config: Option<wgpu::SurfaceConfiguration>,
    // ...
}
```

Update `GpuState::new()` to wrap in `Some()`, and update all call sites that access `self.surface` and `self.surface_config` to use `.as_ref().unwrap()` or handle the `None` case.

Add `GpuState::new_headless()`:

```rust
pub fn new_headless(
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: wgpu::Adapter,
    instance: wgpu::Instance,
    resolution: Resolution,
    phosphor: &PhosphorType,
) -> Self {
    // Initialize all pipelines at the given resolution
    // Composite pipeline uses Rgba16Float (no surface to query)
    let composite = CompositePipeline::new(&device, wgpu::TextureFormat::Rgba16Float);
    // ... initialize other pipelines same as new() but with no surface ...

    Self {
        surface: None,
        surface_config: None,
        // ... all other fields ...
    }
}
```

This is a significant refactor — all the existing code that does `self.surface.get_current_texture()`, `self.surface.configure()`, etc. needs to be gated on `self.surface.is_some()`. The render method should handle the headless case (no swapchain texture acquisition, no present).

- [ ] **Step 4: Update existing call sites for Option<Surface>**

Key places that access surface directly:

- `render()` line 273: `self.surface.get_current_texture()` → gate on `self.surface.as_ref()`
- `resize()` line 211: `self.surface_config.width = width` → gate on `self.surface_config.as_mut()`
- `resize()` line 217: `self.surface.configure()` → gate on both

- [ ] **Step 5: Verify it compiles**

Run: `cargo check`
Run: `cargo run -- --help` (should show CLI options)
Run: `cargo run` (should launch normally in UI mode)

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs src/main.rs src/gpu/mod.rs
git commit -m "feat: add CLI headless mode skeleton with surface-optional GpuState

Add clap-based CLI with --record, --audio, --resolution, --fps,
--phosphor, --preset flags. GpuState surface/surface_config become
Option<> for headless operation. Headless render loop is scaffolded
but not yet fully functional (needs GpuState::new_headless implementation)."
```

---

### Task 12: Complete headless GpuState and render loop

**Files:**

- Modify: `src/gpu/mod.rs`
- Modify: `src/cli.rs`

Flesh out `GpuState::new_headless()` with full pipeline initialization and complete the headless render loop.

- [ ] **Step 1: Implement `GpuState::new_headless()`**

Copy the pipeline initialization from `GpuState::new()` but skip surface-related setup. The key differences:

- No surface creation or configuration
- Composite pipeline uses `Rgba16Float` target format
- No egui renderer (headless doesn't need it)
- No profiler (optional)

Make `egui_renderer` an `Option<egui_wgpu::Renderer>` or initialize with a dummy. The simplest approach: keep it as non-optional and create it with `Rgba16Float` format (it won't be used in headless mode but avoids Option complexity).

- [ ] **Step 2: Complete the headless render loop in `src/cli.rs`**

```rust
// In run_headless():

// Create GpuState headless
let phosphor = PhosphorType::from_name(&cli.phosphor)
    .ok_or_else(|| anyhow::anyhow!("Unknown phosphor type: {}", cli.phosphor))?;
let mut gpu = GpuState::new_headless(device, queue, adapter, instance, resolution, &phosphor);

// Set beam parameters
gpu.beam_params.intensity = cli.intensity;
gpu.beam_params.focus = cli.focus;
// Set composite to HDR passthrough
gpu.composite_params.set_tonemap_mode(TonemapMode::None);

// Create input state for audio
let mut input = InputState::new();
input.mode = InputMode::Audio;
// Load audio into input state...

// Create recording state
let config = RecordingConfig {
    width, height,
    fps: cli.fps,
    output_path: output_path.clone(),
    audio_path: Some(audio_path.clone()),
    preset,
    custom_args: cli.ffmpeg_args.clone(),
    pre_roll_frames: pre_roll,
    total_frames,
};
let mut recording = RecordingState::start(&gpu.device, config, decoded.sample_rate as f32)?;

// Render loop
let samples_per_frame = recording.samples_per_frame;
let total_with_preroll = total_frames + pre_roll as u64;

for frame_idx in 0..total_with_preroll {
    if cancelled.load(Ordering::Relaxed) {
        eprintln!("\nCancelled.");
        break;
    }

    // Generate samples — use recording-specific audio method that reads
    // sequentially through decoded buffer (not cpal-driven)
    let samples = input.generate_audio_samples_recording(
        cli.focus, 1.0, width as f32,
        samples_per_frame,
    );

    // Render to offscreen texture
    gpu.render_offscreen(&samples, recording.dt, &recording.offscreen_view);

    // Readback and pipe (skip during pre-roll)
    if !recording.is_pre_roll() {
        let mut encoder = gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("readback") },
        );
        recording.readback.copy_from_texture(&mut encoder, &recording.offscreen_texture);
        gpu.queue.submit(std::iter::once(encoder.finish()));

        let mut frame_data = Vec::new();
        recording.readback.read_mapped(&gpu.device, |data| {
            frame_data.extend_from_slice(data);
        });
        recording.pipe.write_frame(&frame_data, height)?;
    }

    recording.advance_frame();

    // Progress
    if !recording.is_pre_roll() {
        eprint!("\rframe {}/{} ({:.1}%)",
            recording.current_frame, total_frames,
            recording.progress() * 100.0);
    } else {
        eprint!("\rpre-roll {}/{}",
            pre_roll - recording.pre_roll_remaining, pre_roll);
    }
}

eprintln!();
let status = recording.finish()?;
eprintln!("Done. ffmpeg exited with: {status}");
```

- [ ] **Step 3: Add `render_offscreen()` method to GpuState**

A simplified render method that skips swapchain and egui:

```rust
pub fn render_offscreen(
    &mut self,
    samples: &[BeamSample],
    dt: f32,
    target: &wgpu::TextureView,
) {
    let mut encoder = self.device.create_command_encoder(
        &wgpu::CommandEncoderDescriptor { label: Some("offscreen_render") },
    );

    // Same pipeline passes as render(), but composite targets the offscreen view
    // and no egui pass
    self.beam_write.write(/* ... */);
    self.spectral_resolve.render(/* ... */);
    self.decay.decay(/* ... */);
    self.faceplate_scatter.render(/* ... */);
    self.composite.render(&self.device, &mut encoder, target, /* ... */);

    self.queue.submit(std::iter::once(encoder.finish()));
}
```

- [ ] **Step 4: Test headless recording**

Run: `cargo run -- --record /tmp/test.mp4 --audio path/to/test.wav --resolution 1920x1080 --fps 60 --phosphor P1`
Expected: Renders all frames, produces a valid MP4 file.

- [ ] **Step 5: Commit**

```bash
git add src/gpu/mod.rs src/cli.rs
git commit -m "feat: complete headless recording render loop

Implement GpuState::new_headless() with full pipeline initialization
and render_offscreen() for headless frame rendering. CLI mode now
produces complete video output with progress reporting and SIGINT
handling."
```

---

### Task 13: Final polish and manual testing

**Files:**

- Various small fixes across all modified files

- [ ] **Step 1: Test oscilloscope mode recording (UI)**

Run: `cargo run`
Test: Oscilloscope mode → Recording tab → set 10s duration → 1080p60 → h265-hdr10 → Record
Expected: Progress bar updates, output file produced, preview visible.

- [ ] **Step 2: Test audio mode recording (UI)**

Test: Load an audio file → Recording tab → 1080p60 → h265-hdr10 → Record
Expected: Video with synced audio, correct duration.

- [ ] **Step 3: Test cancellation**

Test: Start recording → Cancel midway
Expected: Partial file is finalized and playable.

- [ ] **Step 4: Test 4K resolution**

Test: Record at 3840x2160
Expected: Video is correct resolution, no artifacts.

- [ ] **Step 5: Test all presets**

Test each: h265-hdr10, h265-sdr, av1-hdr10, prores-4444
Expected: All produce valid output files.

- [ ] **Step 6: Test headless CLI**

Run: `cargo run -- --record /tmp/test.mp4 --audio test.wav`
Expected: Renders, shows progress, produces video.

- [ ] **Step 7: Test ffmpeg not found**

Temporarily remove ffmpeg from PATH and try recording.
Expected: Clear error message in UI.

- [ ] **Step 8: Fix any issues found during testing**

- [ ] **Step 9: Final commit**

```bash
git add -A
git commit -m "fix: polish recording flow after integration testing

Fix issues found during manual testing of the video recording pipeline."
```

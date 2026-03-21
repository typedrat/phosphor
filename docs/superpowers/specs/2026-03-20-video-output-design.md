# Video Output — Design Spec

## Overview

Add offline video recording to Phosphor, enabling high-quality video export of CRT simulations with perfect frame timing. Primary use case is oscilloscope music videos with synced audio, but oscilloscope, audio, and vector input modes are supported through the UI. External mode is out of scope for v1 (its asynchronous sample arrival doesn't fit the batch-on-demand model). A headless CLI mode supports audio input for scripted/batch rendering.

## Offline Render Loop

When recording starts, the app enters a **recording state** that changes frame loop behavior:

- **Frame pacing**: `ControlFlow::Poll` instead of `WaitUntil` — render as fast as possible, no vsync gating.
- **Sim thread mode**: Switches from real-time pacing to **batch-on-demand** — produces exactly `sample_rate / fps` samples per frame, then signals ready. Synchronization uses a `crossbeam_channel` pair: the render thread sends a "next frame" request, the sim thread receives it, generates the batch, and sends the samples back via a second channel. The render thread blocks on the response channel.
- **Sample flow**: The ring buffer (`rtrb`) is bypassed during recording. The channel-based request/response guarantees exactly the right number of samples per frame — no drift, no dropped samples.
- **Frame counter**: Tracks current frame / total frames. Total = `duration * fps` for oscilloscope/vector, `audio_length * fps` for audio. Recording ends when all frames are rendered.
- **Duration (oscilloscope/vector)**: The Recording tab includes a duration field (seconds) that the user sets before recording. This is mandatory for oscilloscope mode (which generates infinite samples) and vector mode (which loops its display list). For audio mode, duration is derived from the audio file length.
- **Fixed dt**: Each frame uses `dt = 1.0 / fps`, passed to the decay pass. Deterministic regardless of wall time.
- **Pre-roll**: Before piping frames to ffmpeg, render a configurable number of warm-up frames (default: `fps` frames, i.e. 1 second) that are rendered but not piped. This allows the accumulation buffer to build up phosphor decay history so the recording doesn't start from a cold black screen, which is especially visible with slow-decay phosphors like P7 and P14. For audio mode, pre-roll uses the beginning of the audio signal — the audio position rewinds to t=0 after pre-roll completes, so the recorded video starts from the beginning of the audio. This requires the audio decoder to support seeking to the start, which symphonia supports for all formats we use.
- **UI responsiveness**: The winit event loop drives recording — one frame per `RedrawRequested`, then immediately request another redraw. The event loop still processes UI events between frames (progress updates, cancel button).
- **Cancellation**: User hits stop in the UI. On next `RedrawRequested`, recording tears down — closes the ffmpeg pipe (finalizing the file), switches back to real-time mode.

## Render Target & Readback

### Offscreen Texture

When recording starts, create a `wgpu::Texture` at the configured output resolution with:

- Format: `Rgba16Float` — always HDR. The composite pass runs normally (glass tint, curvature, edge falloff, faceplate scatter blending) except tonemapping is set to `None` (HDR passthrough). SDR presets include ffmpeg filter chains for tonemapping (see Presets section).
- Usage: `RENDER_ATTACHMENT | COPY_SRC`
- Resolution limit: Textures (HDR buffer, faceplate scatter, offscreen target) are clamped to `max_texture_dimension_2d` (typically 8192). The 8K preset (7680x4320) is within this limit. Custom resolutions are clamped to `min(8192, device.limits().max_texture_dimension_2d)` in the UI. The accumulation buffer (a flat storage buffer) is bounded by `max_storage_buffer_binding_size` instead, which is much larger.
- VRAM note: At 8K, the offscreen texture (~253 MB), staging buffer (~253 MB), HDR buffer (~506 MB at `Rgba32Float`), and accumulation buffer (layers x ~127 MB) add up. Phosphors with many decay layers (P7, P14) at 8K may require 2+ GB VRAM. The UI should display estimated VRAM usage before recording starts.

The composite pass targets the offscreen texture instead of the swapchain. All other passes (beam write, spectral resolve, decay, faceplate scatter) write to their usual buffers/textures, which are re-created at the recording resolution. Since the `CompositePipeline` bakes the target format into its render pipeline at construction, it must be recreated when entering recording mode (targeting `Rgba16Float`) and again when exiting (targeting the swapchain format).

### Staging Buffer

A `wgpu::Buffer` with usage `COPY_DST | MAP_READ`. Size is `padded_bytes_per_row * height`, where `bytes_per_row = width * 8` (4 channels x 2 bytes per `f16`) and `padded_bytes_per_row` is rounded up to `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (256 bytes). When writing to the ffmpeg pipe, each row must be trimmed to `width * 8` bytes to strip the padding. After the composite pass, `copy_texture_to_buffer` copies the offscreen texture into the staging buffer.

### Readback

After command submission: `buffer.slice(..).map_async()` + `device.poll(Wait)` to synchronously wait for the copy. Read the mapped bytes, write to the ffmpeg pipe (stripping row padding). Unmap for the next frame.

**Known limitation**: Synchronous readback stalls the GPU while the CPU reads and pipes data. A double-buffered approach (two staging buffers, map frame N-1 while GPU renders frame N) would improve throughput but adds complexity. This is a v2 optimization — single-buffer is correct and simpler for v1.

### Preview Blit

After the offscreen render, a fullscreen-triangle render pass samples the offscreen texture with bilinear filtering and draws it to the swapchain surface at window resolution. This reuses the same fullscreen-triangle pattern as the composite pass but with a simple texture-sample shader. The egui overlay renders on top of the swapchain as usual.

### Cleanup

When recording ends, the offscreen texture, staging buffer, and resized accumulation buffer are dropped. The pipeline reverts to rendering at window resolution to the swapchain.

## FFmpeg Pipe

### Process Spawning

Spawn `ffmpeg` as a child process with stdin piped:

```
ffmpeg -y -f rawvideo -pix_fmt rgbaf16le -s {width}x{height} -r {fps}
       -i pipe:0
       [-i {audio_file_path}]    # only for audio input mode
       [-c:a copy]               # pass through audio codec if container supports it, else -c:a aac
       {codec_args_from_preset}
       {output_path}
```

- `rgbaf16le`: 16-bit half-float RGBA little-endian, matching `Rgba16Float` memory layout. Requires ffmpeg 5.0+ with float rawvideo support. The Nix flake pins a compatible ffmpeg version. For non-Nix users, the ffmpeg availability check (see below) also verifies `rgbaf16le` support by running `ffmpeg -pix_fmts` and checking the output; if unsupported, the error message tells the user to install ffmpeg >= 5.0.
- Audio file passed as a second input and muxed directly. Audio codec is selected based on a static container/codec compatibility table: if the source codec is supported by the output container (e.g. AAC in MP4, FLAC in MKV), use `-c:a copy`; otherwise use `-c:a aac` for MP4/MKV or `-c:a pcm_s16le` for MOV/ProRes workflows. This avoids a trial-and-error spawn cycle.
- Codec args determined by preset or custom override.
- Output container is inferred by ffmpeg from the file extension. Each preset documents its recommended extension (`.mp4` for H.265, `.mkv` for AV1, `.mov` for ProRes). The UI auto-suggests the extension when a preset is selected, but the user can override it.

### FFmpeg Availability

Before starting a recording, check that `ffmpeg` is on PATH (via `which ffmpeg` or `Command::new("ffmpeg").arg("-version")`). If not found, surface a clear error in the UI: "ffmpeg not found. Install ffmpeg or use the Nix package." This covers users building with plain `cargo` outside Nix.

### Presets

| Slug          | Description                                | Key ffmpeg args                                                                                                            | Use Case                              |
| ------------- | ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| `h265-hdr10`  | H.265 Main 10, PQ transfer, HDR10 metadata | `-c:v libx265 -pix_fmt yuv420p10le -x265-params "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc"`                | High quality HDR playback             |
| `h265-sdr`    | H.265, SDR, broad compatibility            | `-vf "zscale=t=linear,tonemap=hable,zscale=t=bt709:p=bt709:m=bt709" -c:v libx265 -pix_fmt yuv420p`                         | General sharing                       |
| `av1-hdr10`   | AV1 with HDR10, best compression           | `-c:v libsvtav1 -pix_fmt yuv420p10le -svtav1-params "color-primaries=9:transfer-characteristics=16:matrix-coefficients=9"` | Quality + small files (slower encode) |
| `prores-4444` | ProRes 4444                                | `-c:v prores_ks -profile:v 4444 -pix_fmt yuva444p10le`                                                                     | Editing / lossless workflow           |

SDR presets include a tonemapping filter chain (`zscale` + `tonemap`) that converts the linear HDR input to SDR. HDR presets pass through the HDR data with appropriate color metadata.

A custom ffmpeg args text field overrides the preset entirely when populated.

### Writing Frames

Each frame, the mapped staging buffer bytes are written to the child process's stdin. Blocking write, bounded data volume per frame.

### Finalization

Close stdin when recording ends (all frames rendered or cancelled). This signals ffmpeg to flush and finalize the container. Wait for child process exit.

### Error Handling

If the ffmpeg process dies mid-recording (bad codec args, disk full, etc.), detect on next write (broken pipe), surface the error in the UI, end recording gracefully.

### Nix Integration

`ffmpeg` added to the package's runtime dependencies via `flake.nix`, guaranteed available on PATH. The flake pins an ffmpeg version >= 5.0 with `rgbaf16le` support.

## Recording Tab UI

A third tab in the side panel alongside Scope and Engineer.

### Output Settings

- **Resolution**: Dropdown with presets (720p, 1080p, 1440p, 4K, 8K) plus custom width/height fields. Custom values clamped to `min(8192, device.limits().max_texture_dimension_2d)` in both dimensions.
- **FPS**: Dropdown (24, 30, 60, 120) plus custom.
- **Duration**: Seconds field, required for oscilloscope and vector modes. Hidden for audio mode (derived from file length). For vector mode, this is the total recording duration (the display list loops).
- **Output path**: File picker via `rfd`.

### Encoding Settings

- **Preset dropdown**: `h265-hdr10`, `h265-sdr`, `av1-hdr10`, `prores-4444`.
- **Custom ffmpeg args**: Text field for power users. Overrides preset when populated.

### Controls

- **Record button**: Starts recording. Greyed out until output path is set and an input source is active (and duration is set for oscilloscope/vector modes).
- **Progress bar**: Current frame / total frames, percentage, elapsed time.
- **Cancel button**: Stops recording, finalizes partial file.

### State Gating

While recording, **all controls in the Scope and Engineer tabs are disabled** — phosphor type, input mode, intensity, focus, beam parameters, decay settings, faceplate scatter, tonemapping, and resolution scale are all frozen. Changing any of these mid-recording would require re-creating GPU pipeline state (accumulation buffer, emission params, etc.) and produce inconsistent output. The Recording tab stays active for progress and cancellation.

## CLI Headless Mode

Audio-only for v1. Runs without a window — no surface, no egui, no event loop.

### Usage

```
phosphor --record output.mp4 --audio input.wav [options]
```

### Options

| Flag                  | Default        | Description                                         |
| --------------------- | -------------- | --------------------------------------------------- |
| `--resolution WxH`    | 1920x1080      | Output resolution (max 8192x8192)                   |
| `--fps N`             | 60             | Frame rate                                          |
| `--phosphor TYPE`     | P1             | Phosphor type                                       |
| `--preset SLUG`       | h265-hdr10     | Encoding preset                                     |
| `--ffmpeg-args "..."` | —              | Override preset with custom args                    |
| `--intensity F`       | default        | Beam intensity                                      |
| `--focus F`           | default        | Beam focus                                          |
| `--pre-roll N`        | fps (1 second) | Number of warm-up frames to render before recording |

### Execution Model

Simple loop — no winit, no event loop. Generate samples, render, readback, pipe, repeat. Progress printed to stderr (`frame 1234/5678 (21.7%)`). Exits on completion or SIGINT.

### SIGINT Handling

Install a signal handler via the `ctrlc` crate. On SIGINT, set an atomic flag that the render loop checks each frame. When set, the loop exits cleanly — closes the ffmpeg pipe (finalizing the partial file) and exits with code 0.

### Surface-Optional GPU Init

`GpuState` must be constructable without a surface. This is a non-trivial refactor: `surface` and `surface_config` (currently non-optional fields) become `Option<...>`, and a `GpuState::new_headless(width, height)` constructor is added alongside the existing `new(window)`. Call sites that access `surface`/`surface_config` need to handle the `None` case (skip swapchain operations in headless mode). The composite pipeline uses `Rgba16Float` as its target format (hardcoded, not queried from a surface).

### Argument Parsing

`clap` crate for CLI parsing. When `--record` is present, skip winit entirely.

## Dependencies

| Dependency        | Type        | Purpose                           |
| ----------------- | ----------- | --------------------------------- |
| `clap`            | Cargo (new) | CLI argument parsing              |
| `ctrlc`           | Cargo (new) | SIGINT handling for headless mode |
| `ffmpeg` (>= 5.0) | Nix runtime | Video encoding via pipe           |

## Files Affected

| File                        | Changes                                                                                                  |
| --------------------------- | -------------------------------------------------------------------------------------------------------- |
| `src/gpu/mod.rs`            | Offscreen texture, staging buffer, readback, surface-optional init (`Option<Surface>`, `new_headless()`) |
| `src/gpu/preview_blit.rs`   | New — preview blit pipeline (fullscreen triangle, bilinear sample from offscreen texture to swapchain)   |
| `src/gpu/preview_blit.wgsl` | New — simple texture-sample shader for preview blit                                                      |
| `src/gpu/accumulation.rs`   | Re-creation at recording resolution                                                                      |
| `src/app.rs`                | Recording state, frame loop mode switching, `ControlFlow::Poll`                                          |
| `src/simulation.rs`         | Batch-on-demand mode with channel-based synchronization                                                  |
| `src/ui/mod.rs`             | Recording tab registration                                                                               |
| `src/ui/recording_panel.rs` | New — recording tab UI (output settings, encoding, controls, progress)                                   |
| `src/main.rs`               | Clap args, headless vs windowed entry path                                                               |
| `Cargo.toml`                | Add `clap`, `ctrlc`                                                                                      |
| `flake.nix`                 | Add `ffmpeg` to runtime deps                                                                             |

# Phosphor — X-Y CRT Simulator

## Project Overview

A physically-based X-Y CRT simulator in Rust using wgpu for GPU compute/rendering and egui for the UI. The goal is to accurately model the physics of phosphor emission, decay, and CRT electron beam behavior — not just approximate the visual appearance.

## Build & Run

```bash
cargo run              # Debug build
cargo run --release    # Release build (recommended for performance)
RUST_LOG=debug cargo run  # With debug logging
```

## Architecture

### Core Rendering Pipeline (GPU)

Accumulation buffer architecture — the phosphor screen state lives entirely on the GPU as a flat storage buffer of scalar energy layers. Each frame the pipeline runs these passes in order:

1. **Beam Write Pass** (compute shader): Splats new beam hits as Gaussian spots, depositing scalar energy into accumulation layers per decay term. Tier-1 instantaneous terms are folded into a one-frame scalar layer.
2. **Spectral Resolve Pass** (fragment shader): Reads accumulation layers, applies per-group emission weights, integrates spectral energy via CIE weights to produce XYZ → linear sRGB into the HDR buffer. Runs _before_ decay so newly deposited energy is visible at full brightness.
3. **Decay Pass** (compute shader): Applies tier-aware decay to the accumulation buffer — multiplicative `exp(-dt/τ)` for slow exponentials (tier 2), elapsed-time tracking for power-law terms (tier 3), and clears tier-1 instant layers.
4. **Faceplate Scatter Pass** (compute shader): Downsample HDR → half-resolution, then separable Gaussian blur (horizontal + vertical) for halation/bloom.
5. **Composite Pass** (fragment shader): Combines HDR + faceplate scatter → applies glass tint, curvature, edge falloff, tonemapping → final display output.
6. **egui Overlay Pass** (render pass): Side panel / controls drawn on top of the CRT output.

### Accumulation Buffer

A flat `wgpu::Buffer` of `u32` values (bitcast to/from `f32`) indexed as `layer * (width * height) + y * width + x`. The number of layers is variable per phosphor, determined by decay term classification:

- **Tier 2 layers**: One scalar layer per slow exponential term
- **Tier 3 layers**: 2 layers if power-law present (peak energy + elapsed time)
- **Tier 1 layer**: 1 layer if instantaneous terms present (cleared each frame after spectral resolve)

### Spectral Representation

- `SPECTRAL_BANDS` (compile-time constant, 16) bands at ~25nm intervals across 380–780nm
- Changing this constant propagates through the entire pipeline (Rust const + WGSL pipeline-overridable constant)
- Emission weights are stored per emission group in `SpectralResolveParams`, not per accumulation layer — the accumulation buffer stores scalar energy, and spectral weighting happens at resolve time
- CIE 1931 XYZ integration weights are pre-computed per band

### Phosphor Model

Each phosphor type (P1, P2, P7, P11, P31, etc.) is defined by:

- Per-layer spectral emission curve (normalized weights across `SPECTRAL_BANDS`, Gaussian approximation from peak wavelength and FWHM)
- Decay terms: a `Vec<DecayTerm>` with up to 8 terms, each either `Exponential { amplitude, tau }` or `PowerLaw { amplitude, alpha, beta }`
- Dual-layer support for phosphors with distinct fluorescence/phosphorescence (P2, P7, P14, etc.)
- Peak wavelength, relative luminance, relative writing speed

Phosphor data is baked at compile time from `data/phosphors.toml` via a proc macro (`phosphor-data-macro`), with runtime loading also supported.

### Three-Tier Hybrid Decay Model

Based on Kuhn (2002) PMT measurements and Selomulya (2003) kinetic data:

| Tier | Condition              | Mechanism                               | Implementation                                                        |
| ---- | ---------------------- | --------------------------------------- | --------------------------------------------------------------------- |
| 1    | τ < 100µs              | Instantaneous (decays within one frame) | Integrated analytically during beam write, stored as one-frame scalar |
| 2    | τ ≥ 100µs, exponential | Slow multiplicative decay               | `value *= exp(-dt/τ)` per frame in accumulation buffer                |
| 3    | Power-law              | Bimolecular DAP recombination           | Tracks per-texel elapsed time, evaluates `peak * (α/(t+α))^β`         |

### Beam Model

- Gaussian core + halo spot profile: `I(r) = (1-h)·exp(-r²/2σ²) + h·exp(-r²/2σ_halo²)`
- Spot size affected by focus setting, acceleration voltage, and beam current (space charge)
- Arc-length resampling decouples energy deposition from input sample rate — consecutive samples are merged to ~0.5× beam sigma spacing for uniform trace brightness

### Input Modes

All input modes produce a common `BeamSample { x, y, intensity, dt }` stream:

1. **Oscilloscope**: Built-in signal generators (sine, triangle, square, sawtooth, noise) for X/Y channels
2. **Audio**: Stereo audio file where L=X, R=Y (for oscilloscope music). Uses symphonia for decoding.
3. **Vector**: Display list of line segments `(x0, y0, x1, y1, intensity)` loaded from JSON
4. **External**: Text protocol over stdin/Unix socket (`B x y intensity dt`, `L x0 y0 x1 y1 intensity`, `F`)

### CRT Effects (Composite Pipeline)

- Spectral → CIE XYZ integration (pre-computed per-band weights)
- XYZ → sRGB with luminance-preserving gamut mapping
- Faceplate scatter / halation (half-res separable Gaussian blur of thresholded HDR)
- Glass faceplate tint (RGB transmission curve)
- Screen curvature distortion (parameterized radius)
- Edge brightness falloff (Lambertian cosine)
- Tonemapping (Reinhard, ACES, Clamp, or HDR passthrough)
- HDR surface format support (Rgba16Float when the display supports it)

### UI (egui)

Two tabs in a side panel:

- **Scope Mode**: Phosphor type, input mode, intensity/focus knobs, per-mode controls (waveform params, audio transport, file pickers, etc.)
- **Engineer Mode**: Raw physics parameters — beam spot profile, decay term display with tier classification, faceplate scatter, glass/curvature/falloff, tonemapping, resolution scale, GPU timing plots

Supports combined (single window) and detached (CRT viewport + controls as separate OS windows) layouts. Toggle with `Ctrl+D`. Fullscreen with `Ctrl+F`.

## Module Structure

```
src/
  main.rs              — entry point, window creation, event loop, multi-window
  app.rs               — InputState, per-mode state (oscilloscope, audio, vector, external)
  types.rs             — Resolution type
  phosphor/
    mod.rs             — phosphor database (compile-time baked + runtime loading)
    spectral.rs        — spectral band definitions, CIE integration weights
  beam/
    mod.rs             — BeamSample, BeamSource trait, SPSC sample channel
    oscilloscope.rs    — signal generators
    vector.rs          — display list input
    audio.rs           — audio file decoding via symphonia, L/R → X/Y
    external.rs        — pipe/socket protocol parser (nom-based)
    resample.rs        — arc-length resampling for uniform beam energy deposition
  gpu/
    mod.rs             — GpuState: device/queue setup, pipeline orchestration, render loop
    accumulation.rs    — flat storage buffer, HdrBuffer, layer count computation
    beam_write.rs      — BeamWritePipeline, BeamParams, EmissionParams
    beam_write.wgsl    — compute shader: Gaussian splat into scalar accumulation layers
    decay.rs           — DecayPipeline, DecayParams, DecayTermGpu
    decay.wgsl         — compute shader: three-tier decay (exp + power-law + instant clear)
    spectral_resolve.rs — SpectralResolvePipeline, SpectralResolveParams, EmissionGroupGpu
    spectral_resolve.wgsl — fragment shader: scalar layers × emission weights → CIE XYZ → sRGB
    faceplate_scatter.rs — FaceplateScatterPipeline, downsample + separable blur
    faceplate_scatter_downsample.wgsl — compute: HDR → half-res with threshold
    faceplate_scatter_blur.wgsl — compute: separable Gaussian blur (H then V)
    composite.rs       — CompositePipeline, CompositeParams, TonemapMode
    composite.wgsl     — fragment shader: HDR + scatter → glass/curvature/tonemap → display
    profiler.rs        — GPU timestamp query profiler, timing history
  ui/
    mod.rs             — UiState, egui integration, combined/detached window management
    scope_panel.rs     — scope-style controls (phosphor, input mode, intensity, focus)
    engineer_panel.rs  — physics parameter controls, emission spectrum plot, GPU timing plot
crates/
  cie-data/            — CIE 1931 2° observer data (compile-time)
  phosphor-data/       — PhosphorType, PhosphorLayer, DecayTerm, spectral utilities, TOML loading
  phosphor-data-macro/ — proc macro that bakes data/phosphors.toml into a static array
data/
  phosphors.toml       — phosphor database (JEDEC types with explicit decay terms)
  CIE_xyz_1931_2deg.csv — CIE color matching function data
```

## Key Dependencies

| Crate                                      | Purpose                                         |
| ------------------------------------------ | ----------------------------------------------- |
| `winit` (0.30)                             | Window management, event loop, multi-window     |
| `wgpu` (27)                                | GPU compute and render pipelines                |
| `egui` + `egui-wgpu` + `egui-winit` (0.33) | UI framework (manual integration, not eframe)   |
| `egui_extras` + `egui_plot`                | File dialog integration, data plots             |
| `symphonia`                                | Audio file decoding (WAV, FLAC, OGG, MP3)       |
| `glam`                                     | Vector/matrix math                              |
| `bytemuck`                                 | Zero-copy GPU buffer casting                    |
| `rtrb`                                     | Lock-free SPSC ring buffer for beam samples     |
| `nom`                                      | Parser combinators for external protocol        |
| `strum`                                    | Enum derive macros (Display, EnumCount, etc.)   |
| `rfd`                                      | Native file dialogs                             |
| `pollster`                                 | Blocking async executor for wgpu initialization |

**Note:** We use individual egui/winit/wgpu components instead of eframe. The manual integration gives us full control over the wgpu pipeline. egui 0.33 requires wgpu 27 and winit 0.30 — these versions must stay in sync.

## Conventions

- Shaders are WGSL, stored as `.wgsl` files in `src/gpu/`, included via `include_str!`
- GPU buffer structs derive `bytemuck::Pod` and `bytemuck::Zeroable`
- The spectral band count is a compile-time constant `SPECTRAL_BANDS` — changing it should require no other code changes (propagates via Rust const + WGSL pipeline-overridable constant)
- Phosphor data is defined in `data/phosphors.toml` and baked at compile time via `phosphor_data_macro::phosphor_table!`
- All coordinates are normalized [0, 1] internally; display mapping happens in the viewport
- GPU pipeline structs store bind group layouts; buffers are created per-frame via `DeviceExt::create_buffer_init`
- `TonemapMode` uses `#[repr(u32)]` enum with WGSL `alias TonemapMode = u32;` + typed constants

## Threading Model

- **Main thread**: winit event loop, egui rendering, GPU command submission, beam sample generation
- Input sources run synchronously on the main thread during `RedrawRequested`, generating samples for the current frame
- SPSC ring buffer (`rtrb`) available for future threaded input sources

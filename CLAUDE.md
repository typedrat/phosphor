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

Accumulation buffer architecture — the phosphor screen state lives entirely on the GPU as spectral energy textures:

1. **Beam Write Pass** (compute shader): Splats new beam hits as Gaussian spots into spectral accumulation textures, depositing energy per spectral band
2. **Decay Pass** (compute shader): Applies per-texel multi-exponential decay (`A₁·exp(-t/τ₁) + A₂·exp(-t/τ₂)`) to each spectral band independently
3. **Tonemap/Display Pass** (fragment shader): Spectral bands → CIE XYZ → linear sRGB → bloom → glass effects → curvature → tonemapped sRGB output

### Spectral Representation

- `SPECTRAL_BANDS` (compile-time constant, initially 16) bands at ~25nm intervals across 380-780nm
- Changing this constant should propagate through the entire pipeline (Rust const + WGSL define)
- Two decay components (fast/slow) × up to two layers (fluorescence/phosphorescence) per phosphor type
- Buffer layout: multiple RGBA32Float textures, 4 spectral bands per texture

### Phosphor Model

Each phosphor type (P1, P2, P7, P11, P31, etc.) is defined by:

- Per-layer spectral emission curve (weights across SPECTRAL_BANDS)
- Multi-exponential decay constants fitted to Tektronix data (time to 10%, 1%, 0.1%)
- Dual-layer support for phosphors with distinct fluorescence/phosphorescence colors (P2, P7, P14, etc.)
- Peak wavelength, relative luminance, relative writing speed

Reference data is in `docs/` — Tektronix CRT Data sheets from 1966.

### Beam Model

- Gaussian core + halo spot profile: `I(r) = (1-h)·exp(-r²/2σ²) + h·exp(-r²/2σ_halo²)`
- Spot size affected by focus setting, acceleration voltage, and beam current (space charge)
- Line drawing subdivides segments for correct dwell-time energy distribution

### Input Modes

All input modes produce a common `BeamSample { x, y, intensity, dt }` stream:

1. **Oscilloscope**: Built-in signal generators (sine, triangle, square, sawtooth, noise) for X/Y channels
2. **Audio**: Stereo audio file where L=X, R=Y (for oscilloscope music). Uses symphonia for decoding.
3. **Vector**: Display list of line segments `(x0, y0, x1, y1, intensity)`
4. **External**: Text protocol over stdin/Unix socket (`B x y intensity dt`, `L x0 y0 x1 y1 intensity`, `F`)

### CRT Effects (Display Pipeline)

- Spectral → CIE XYZ integration (pre-computed per-band weights)
- XYZ → sRGB with gamut mapping
- Bloom convolution (halation from glass faceplate)
- Glass faceplate tint (RGB transmission curve)
- Screen curvature distortion (parameterized radius)
- Edge brightness falloff (Lambertian cosine)
- Filmic tonemapping (HDR → display range)

### UI (egui)

Two tabs in a side/bottom panel:

- **Scope Mode**: Phosphor type, input mode, intensity/focus knobs, per-mode controls (waveform params, audio transport, etc.)
- **Engineer Mode**: Raw physics parameters — beam spot profile, decay constants, spectral curves, glass properties, bloom, tonemap, performance stats

Supports combined (single window) and detached (CRT viewport + controls as separate OS windows) layouts.

## Module Structure

```
src/
  main.rs              — entry point, window creation, event loop
  app.rs               — top-level App state
  phosphor/
    mod.rs             — phosphor database, PhosphorType struct
    spectral.rs        — spectral band definitions, CIE integration weights
    decay.rs           — multi-exponential decay fitting
  beam/
    mod.rs             — BeamSample, sample ring buffer
    oscilloscope.rs    — signal generators
    vector.rs          — display list input
    audio.rs           — audio file decoding, L/R → X/Y
    external.rs        — pipe/socket protocol parser
  gpu/
    mod.rs             — wgpu device/queue setup, pipeline orchestration
    accumulation.rs    — accumulation buffer management
    beam_write.wgsl    — compute shader: splat beam hits
    decay.wgsl         — compute shader: exponential decay
    tonemap.wgsl       — fragment shader: spectral→display
  ui/
    mod.rs             — egui integration, window management
    scope_panel.rs     — scope-style controls
    engineer_panel.rs  — physics parameter controls
    viewport.rs        — CRT viewport widget, graticule overlay
```

## Key Dependencies

| Crate                               | Purpose                                       |
| ----------------------------------- | --------------------------------------------- |
| `winit` (0.31 beta)                 | Window management, event loop, multi-window   |
| `wgpu` (28)                         | GPU compute and render pipelines              |
| `egui` + `egui-wgpu` + `egui-winit` | UI framework (manual integration, not eframe) |
| `symphonia`                         | Audio file decoding (WAV, FLAC, OGG, MP3)     |
| `glam`                              | Vector/matrix math                            |
| `bytemuck`                          | Zero-copy GPU buffer casting                  |

**Note:** We use individual egui/winit/wgpu components instead of eframe because eframe's transitive dependency on winit 0.30 doesn't compile on Rust 1.93+. The manual integration gives us full control over the wgpu pipeline anyway.

## Conventions

- Shaders are WGSL, stored as `.wgsl` files in `src/gpu/`
- GPU buffer structs derive `bytemuck::Pod` and `bytemuck::Zeroable`
- The spectral band count is a compile-time constant `SPECTRAL_BANDS` — changing it should require no other code changes
- Phosphor data is defined in code (not loaded from files) based on the Tektronix reference tables in `docs/`
- All coordinates are normalized [0, 1] internally; display mapping happens in the viewport

## Threading Model

- **Main thread**: winit event loop, egui rendering, GPU command submission
- **Input thread**: Runs active input source, pushes `BeamSample`s into a double-buffered ring buffer

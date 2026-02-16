# Phosphor — Design Overview

A physically-based X-Y CRT simulator. The core principle is to model the actual physics of phosphor emission, electron beam behavior, and CRT optics rather than approximating visual appearance with post-processing tricks.

## Architecture Summary

The system has four major subsystems:

1. **Phosphor Model** (`docs/design-phosphor-model.md`) — Spectral emission, three-tier hybrid decay, dual-layer phosphors
2. **Beam & Input** (`docs/design-beam-input.md`) — Electron beam physics, spot profile, arc-length resampling, and the four input modes
3. **GPU Rendering Pipeline** (`docs/design-gpu-pipeline.md`) — Scalar accumulation buffer, six-pass compute/render pipeline, HDR output
4. **UI** (`docs/design-ui.md`) — egui controls, multi-window support, scope/engineer modes

## Data Flow

```
Input Sources                    GPU Pipeline                         Display
─────────────                    ────────────                         ───────
                                 ┌─────────────────┐
Oscilloscope ─┐                  │  Beam Write      │
Audio File   ─┤→ BeamSample → → │  (compute)       │
Vector List  ─┤   arc-length    │  Gaussian splat  │
External     ─┘   resampled     │  into scalar     │
                                 │  accum layers    │
                                 └────────┬─────────┘
                                          │
                                 ┌────────▼─────────┐
                                 │  Spectral Resolve │
                                 │  (fragment)       │
                                 │  scalar × weights │
                                 │  → CIE XYZ       │
                                 │  → linear sRGB   │  ┌──────────┐
                                 │  → HDR buffer    │  │          │
                                 └────────┬─────────┘  │          │
                                          │            │          │
                                 ┌────────▼─────────┐  │          │
                                 │  Decay            │  │          │
                                 │  (compute)        │  │          │
                                 │  tier 2: exp mul  │  │          │
                                 │  tier 3: pow-law  │  │          │
                                 │  tier 1: clear    │  │          │
                                 └──────────────────┘  │          │
                                                       │          │
                                 ┌──────────────────┐  │          │
                                 │  Faceplate        │  │          │
                                 │  Scatter          │  │          │
                                 │  (compute)        │  │          │
                                 │  downsample       │  │          │
                                 │  + blur H + V     │  │ CRT      │
                                 └────────┬─────────┘  │ Viewport │
                                          │            │          │
                                 ┌────────▼─────────┐  │          │
                                 │  Composite        │  │          │
                                 │  (fragment)       │──│──────────│
                                 │  HDR + scatter    │  │          │
                                 │  → glass tint     │  │          │
                                 │  → curvature      │  │          │
                                 │  → tonemap        │  └──────────┘
                                 └───────────────────┘
```

## Key Design Decisions

- **Scalar accumulation buffer** over spectral-per-layer textures: each decay term gets one scalar energy layer. Spectral weighting happens at resolve time via per-group emission weights, reducing VRAM from O(bands × terms) to O(terms).
- **Three-tier hybrid decay** replaces uniform bi-exponential: instantaneous terms (τ < 100µs) are integrated analytically, slow exponentials use multiplicative decay, and power-law terms track per-texel elapsed time. This matches the actual physics — ZnS phosphors follow power-law kinetics, not exponential.
- **Spectral rendering** with compile-time configurable band count (`SPECTRAL_BANDS`, 16 at ~25nm). Not RGB — this matters for dual-layer phosphors where fluorescence and phosphorescence have different emission spectra.
- **Individual egui/winit/wgpu components** instead of eframe: manual integration gives us full pipeline control. egui 0.33 requires wgpu 27 and winit 0.30.
- **Common BeamSample stream**: all four input modes produce the same `{ x, y, intensity, dt }` events, keeping the GPU pipeline input-agnostic.
- **Arc-length resampling**: decouples energy deposition from input sample rate, preventing periodic brightness modulation along traces.

## Module Map

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
    beam_write.rs/.wgsl — compute: Gaussian splat into scalar accumulation layers
    decay.rs/.wgsl     — compute: three-tier decay (exp + power-law + instant clear)
    spectral_resolve.rs/.wgsl — fragment: scalar layers × emission weights → CIE XYZ → sRGB
    faceplate_scatter.rs + 2 .wgsl — compute: downsample + separable Gaussian blur
    composite.rs/.wgsl — fragment: HDR + scatter → glass/curvature/tonemap → display
    profiler.rs        — GPU timestamp query profiler, timing history
  ui/
    mod.rs             — UiState, egui integration, combined/detached window management
    scope_panel.rs     — scope-style controls (phosphor, input mode, intensity, focus)
    engineer_panel.rs  — physics parameter controls, emission spectrum plot, GPU timing plot
crates/
  cie-data/            — CIE 1931 2° observer data (compile-time)
  phosphor-data/       — PhosphorType, PhosphorLayer, DecayTerm, spectral utilities, TOML loading
  phosphor-data-macro/ — proc macro that bakes data/phosphors.toml into a static array
```

## Threading Model

- **Main thread**: winit event loop, egui rendering, GPU command submission, beam sample generation
- Input sources run synchronously on the main thread during `RedrawRequested`
- SPSC ring buffer (`rtrb`) available for future threaded input sources

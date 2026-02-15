# Phosphor — Design Overview

A physically-based X-Y CRT simulator. The core principle is to model the actual physics of phosphor emission, electron beam behavior, and CRT optics rather than approximating visual appearance with post-processing tricks.

## Architecture Summary

The system has four major subsystems:

1. **Phosphor Model** (`docs/design-phosphor-model.md`) — Spectral emission, multi-exponential decay, dual-layer phosphors
2. **Beam & Input** (`docs/design-beam-input.md`) — Electron beam physics, spot profile, and the four input modes
3. **GPU Rendering Pipeline** (`docs/design-gpu-pipeline.md`) — Accumulation buffers, compute shaders, display pipeline
4. **UI** (`docs/design-ui.md`) — egui controls, multi-window support, scope/engineer modes

## Data Flow

```
Input Sources                    GPU Pipeline                         Display
─────────────                    ────────────                         ───────
                                 ┌─────────────────┐
Oscilloscope ─┐                  │  Beam Write Pass │
Audio File   ─┤→ BeamSample → → │  (compute)       │
Vector List  ─┤   Ring Buffer    │  Gaussian splat  │
External     ─┘                  │  into spectral   │
                                 │  accum textures  │
                                 └────────┬─────────┘
                                          │
                                 ┌────────▼─────────┐
                                 │  Decay Pass       │
                                 │  (compute)        │
                                 │  per-texel        │
                                 │  exp(-dt/τ)       │
                                 └────────┬─────────┘
                                          │
                                 ┌────────▼─────────┐
                                 │  Tonemap Pass     │
                                 │  (fragment)       │    ┌──────────┐
                                 │  spectral → XYZ   │───→│ CRT      │
                                 │  → sRGB → bloom   │    │ Viewport │
                                 │  → glass → curve  │    └──────────┘
                                 │  → tonemap        │
                                 └───────────────────┘
```

## Key Design Decisions

- **Accumulation buffer** over particle system: the phosphor screen state lives as GPU textures. Decay is embarrassingly parallel and cost is bounded by resolution, not beam activity.
- **Spectral rendering** with compile-time configurable band count (`SPECTRAL_BANDS`, initially 16 at ~25nm). Not RGB — this matters for dual-layer phosphors where fluorescence and phosphorescence have different emission spectra.
- **Individual egui/winit/wgpu components** instead of eframe: eframe's winit 0.30 dependency doesn't compile on Rust 1.93+. Manual integration gives us full pipeline control anyway.
- **Common BeamSample stream**: all four input modes produce the same `{ x, y, intensity, dt }` events, keeping the GPU pipeline input-agnostic.

## Module Map

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
    tonemap.wgsl       — fragment shader: spectral → display
  ui/
    mod.rs             — egui integration, window management
    scope_panel.rs     — scope-style controls
    engineer_panel.rs  — physics parameter controls
    viewport.rs        — CRT viewport widget, graticule overlay
```

## Threading Model

- **Main thread**: winit event loop, egui rendering, GPU command submission
- **Input thread**: runs active input source, pushes BeamSamples into a double-buffered ring buffer decoupled from frame rate

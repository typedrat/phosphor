# UI Design

## Framework

egui with manual winit + wgpu integration (not eframe). We own the event loop and wgpu pipeline, and render egui as an overlay pass after our custom CRT rendering.

## Window Layouts

Three modes, switchable via hotkey or menu:

### Combined (default)

Single OS window. The CRT viewport takes the majority of the space, with an egui side panel (resizable) for controls.

### Detached

Two OS windows sharing the same process:

- **CRT Viewport window**: Pure render output, can be fullscreened on any monitor (e.g., a TV).
- **Controls window**: egui-only window with all the control panels.

Both windows share the same `Arc<Mutex<AppState>>`. The viewport window runs the wgpu render pipeline; the controls window is a lightweight egui-only surface. Toggle with `Ctrl+D`.

### Fullscreen

CRT viewport fills the entire screen. Controls hidden. Hotkey (e.g., `Escape` or `Ctrl+D`) to return to combined mode.

## Control Panel

Two tabs:

### Scope Mode Tab

User-facing controls that map to real oscilloscope/CRT concepts:

- **Phosphor type selector**: Dropdown listing all phosphor types (P1, P2, P7, P11, P31, etc.) with their color description.
- **Input mode selector**: Radio buttons — Oscilloscope, Audio, Vector, External.
- **Intensity knob**: Beam current. Maps to energy deposition rate.
- **Focus knob**: Spot size (σ_core).

**Per-input-mode controls** (shown/hidden based on selected mode):

_Oscilloscope:_

- X channel: waveform type, frequency, amplitude, phase, DC offset
- Y channel: same
- Timebase (simulated time / real time)

_Audio:_

- File picker button (opens native file dialog via `rfd` or `egui_extras`)
- Waveform scrubber / seek bar
- Transport: play, pause, stop, loop toggle
- Timebase / speed

_Vector:_

- Load display list (file picker)
- Refresh rate / loop toggle

_External:_

- Mode: stdin / socket
- Socket path (text field, only in socket mode)
- Connection status indicator

**Graticule toggle**: On/off for the 8×10 grid overlay (oscilloscope mode only).

### Engineer Mode Tab

Raw physics parameters for tuning and experimentation:

**Beam:**

- σ_core (spot core width)
- σ_halo (spot halo width)
- Halo fraction (h)
- Space charge coefficient (k)
- Acceleration voltage (kV)

**Phosphor (per-layer overrides):**

- τ_fast, τ_slow
- A_fast / A_slow ratio
- Spectral emission curve viewer (small bar chart of the 16 band weights)

**Display pipeline:**

- Glass tint (RGB color picker)
- Halation bloom radius and intensity
- Screen curvature radius
- Edge falloff strength
- Tonemap curve selector (Reinhard / Filmic / Exposure)
- Tonemap parameters (exposure, white point)

**Accumulation buffer:**

- Resolution multiplier (0.5x, 1x, 2x)
- Precision threshold (minimum energy before zeroing)

**Performance:**

- FPS counter
- GPU time per pass (beam write, decay, tonemap)
- Beam samples per frame
- Active texel count (non-zero)

## egui Rendering Integration

The CRT simulation renders to the swapchain via our custom wgpu pipeline (beam write → decay → tonemap). egui then renders on top as an overlay — the side panel and any floating UI elements are composited over the CRT output. This is the standard approach for mixing egui with a custom 3D/GPU view: egui doesn't own the viewport, it just draws its UI on top. In detached mode, the viewport window skips egui entirely — pure wgpu output only.

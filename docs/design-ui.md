# UI Design

## Framework

egui with manual winit + wgpu integration (not eframe). We own the event loop and wgpu pipeline, and render egui as an overlay pass after the CRT composite pipeline.

## Window Layouts

### Combined (default)

Single OS window. The CRT viewport takes the majority of the space, with an egui side panel (resizable, hideable) for controls. The panel can be toggled with the hamburger button.

### Detached

Two OS windows sharing the same wgpu device/queue:

- **CRT Viewport window**: Pure GPU render output, can be fullscreened on any monitor.
- **Controls window**: egui-only window with all the control panels, using a separate surface and egui renderer with shared `egui::Context`.

Toggle with `Ctrl+D`. Closing the controls window recombines into the main window.

### Fullscreen

CRT viewport fills the entire screen. Toggle with `Ctrl+F`.

## Control Panel

Two tabs:

### Scope Mode Tab

User-facing controls that map to real oscilloscope/CRT concepts:

- **Phosphor type selector**: Dropdown listing all phosphor types with their color description.
- **Input mode selector**: Radio buttons — Oscilloscope, Audio, Vector, External.
- **Intensity knob**: Beam current. Maps to exposure in the composite pipeline.
- **Focus knob**: Spot size (σ_core).

**Per-input-mode controls** (shown/hidden based on selected mode):

_Oscilloscope:_

- X channel: waveform type, frequency, amplitude, phase, DC offset
- Y channel: same
- Sample rate

_Audio:_

- File picker button (native dialog via rfd)
- Transport: play/pause, loop toggle
- Speed control

_Vector:_

- File picker button (native dialog via rfd)
- Beam speed, settling time, loop toggle

_External:_

- Mode: stdin / socket
- Socket path (text field, only in socket mode)
- Connection status indicator

### Engineer Mode Tab

Raw physics parameters for tuning and experimentation:

**Phosphor:**

- Phosphor type selector (shared with scope mode)
- Emission spectrum plot (fluorescence + phosphorescence if dual-layer)
- Decay terms display: read-only summary showing each term's type, tier classification (T1/T2/T3), amplitude, and time constant
- Buffer layer count

**Beam:**

- σ_core (spot core width)
- σ_halo (spot halo width)
- Halo fraction (h)
- Space charge coefficient
- Acceleration voltage (kV)

**Faceplate Scatter:**

- Threshold (minimum brightness to bloom)
- Sigma (blur radius)
- Intensity (blend weight)

**Display pipeline:**

- Tonemap curve selector (Reinhard / ACES / Clamp / None/HDR)
- Exposure
- White point

**Glass Faceplate:**

- Tint (RGB color picker)
- Curvature
- Edge falloff

**Accumulation buffer:**

- Resolution multiplier (0.25x–2x)
- Current resolution display

**Performance:**

- FPS counter
- GPU time per pass (Beam Write, Spectral Resolve, Decay, Faceplate Scatter, Composite)
- Beam samples per frame
- Stacked timing history plot with per-pass color coding

## egui Rendering Integration

The CRT simulation renders to the swapchain via the custom wgpu pipeline (beam write → spectral resolve → decay → faceplate scatter → composite). egui then renders on top as an overlay — the side panel and any floating UI elements are composited over the CRT output. The render pass uses `LoadOp::Load` to preserve the CRT image underneath. In detached mode, the viewport window skips egui entirely — pure wgpu output only. The controls window uses its own egui renderer and surface with a `LoadOp::Clear`.

## Keyboard Shortcuts

| Key      | Action                          |
| -------- | ------------------------------- |
| `Ctrl+D` | Toggle detached controls window |
| `Ctrl+F` | Toggle fullscreen               |
| `Ctrl+Q` | Quit                            |

# Multi-Window Support Design

## Goal

Detachable control panel as a separate OS window. `Ctrl+D` toggles between combined (single window) and detached (two windows) modes.

## Layout Modes

```rust
enum WindowMode {
    Combined,  // single window: CRT viewport + egui side panel
    Detached,  // two windows: CRT-only viewport + egui-only controls
}
```

## Architecture

### App Struct

```
App {
    mode: WindowMode,

    // CRT viewport (always exists)
    viewport_window: Arc<Window>,
    gpu: GpuState,

    // Controls window (only in Detached mode)
    controls_window: Option<Arc<Window>>,
    controls_surface: Option<wgpu::Surface>,
    controls_surface_config: Option<wgpu::SurfaceConfiguration>,
    controls_egui_renderer: Option<egui_wgpu::Renderer>,

    // UI state — always exists, renders to whichever window has the panel
    ui: UiState,
}
```

### Shared Device/Queue

Both windows share the same `wgpu::Device` and `wgpu::Queue` from `GpuState`. The controls window gets its own `Surface` configured against the same adapter. No `Arc<Mutex<>>` needed — winit's event loop is single-threaded, so direct ownership suffices.

### Event Dispatch

`window_event` matches on `window_id`:

- Viewport window: full behavior (egui in combined mode, GPU resize, full render pipeline)
- Controls window: egui events only, lightweight redraw (clear + egui)

### Toggle (Ctrl+D)

- **Combined → Detached**: Create second winit window + surface from existing device. Viewport stops rendering egui overlay.
- **Detached → Combined**: Destroy controls window/surface/renderer. Viewport resumes egui overlay.

### Redraw Flow

- **Combined viewport**: beam write → decay → spectral resolve → scatter → composite → egui overlay → present
- **Detached viewport**: same pipeline, skip egui overlay (`gpu.render(samples, None)`)
- **Detached controls**: clear background → egui render → present

### Window Close

- Closing viewport → exits the app
- Closing controls → recombines (same as Ctrl+D)

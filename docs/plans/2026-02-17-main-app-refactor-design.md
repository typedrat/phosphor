# Main/App Refactor Design

## Problem

`main.rs` is a 680-line monolith containing the `App` struct, `ControlsWindow`, `ApplicationHandler` impl, keyboard shortcut handling, per-frame UI→GPU sync, per-frame UI→sim command dispatch, and controls-window rendering. `app.rs` mixes sim-thread domain types (`InputState`, `AudioState`, `VectorState`) with shared UI/sim types (`InputMode`, `OscilloscopeState`). Adding a new parameter means touching a sync blob in main.rs.

## Goals

1. Reduce main.rs to just the entry point (~15 lines)
2. Decouple UI, GPU, and sim concerns into focused modules
3. Eliminate code duplication between Combined and Detached window modes
4. Clarify which types belong to the sim thread vs the UI thread

## Approach: Extract-and-Focus

Same event-loop architecture, no shared-state changes. Per-frame sync stays explicit but moves into focused functions.

## Module Structure (After)

```
src/
  main.rs              — entry point only: tracing init, EventLoop, run_app
  app.rs               — App struct, ApplicationHandler impl, WindowMode,
                         global shortcut handling, toggle_detach
  controls_window.rs   — ControlsWindow struct, new(), render_egui() helper
  frame.rs             — sync_gpu_params(), dispatch_sim_commands(),
                         drain_samples(), build_sim_frame_info()
  simulation.rs        — SimCommand, InputState, AudioState, VectorState,
                         ExternalState, ExternalMode, sim loop (unchanged)
  types.rs             — Resolution, InputMode, OscilloscopeState
  ui/mod.rs            — UiState with unified run() method
  (gpu/, beam/, phosphor/, etc. — unchanged)
```

## Changes in Detail

### 1. `main.rs` → entry point only

After the refactor, main.rs contains only:

```rust
mod app;
mod beam;
mod controls_window;
mod frame;
mod gpu;
mod phosphor;
mod presets;
mod simulation;
mod simulation_stats;
mod types;
mod ui;

fn main() -> anyhow::Result<()> {
    // tracing setup
    // EventLoop::new()
    // event_loop.run_app(&mut App::default())
}
```

### 2. `app.rs` → App struct + ApplicationHandler

Receives the `App` struct, `WindowMode` enum, `Default` impl, and `ApplicationHandler` impl from main.rs. Methods:

- `resumed()` — window creation, GPU init, sim thread spawn
- `window_event()` — routes events by window ID, calls global shortcut handler
- `handle_viewport_event()` — slimmed down, delegates to `frame.rs` functions
- `handle_controls_event()` — delegates rendering to `ControlsWindow::render()`
- `about_to_wait()` — frame pacing
- `toggle_detach()` — Combined↔Detached toggle

The per-frame sync blob (currently ~60 lines in `handle_viewport_event`) is replaced with calls to `frame::sync_gpu_params()` and `frame::dispatch_sim_commands()`.

### 3. `controls_window.rs` — extracted from main.rs

`ControlsWindow` struct + constructor (currently lines 35–114 of main.rs) plus a `render()` method that takes `&GpuState` and `&mut UiState`.

A free function `render_egui_to_surface()` handles the boilerplate: get texture → update textures → create encoder → update buffers → begin render pass → render → free textures → submit → present. This replaces the ~50-line `render_controls_window` method.

### 4. `frame.rs` — per-frame orchestration

Free functions extracted from `handle_viewport_event`'s `RedrawRequested` arm:

- `sync_gpu_params(gpu: &mut GpuState, ui: &UiState)` — copies beam, scatter, composite params from UI to GPU, applying resolution scaling
- `dispatch_sim_commands(tx: &Sender<SimCommand>, ui: &mut UiState, sample_rate: &mut f32, consumer: &mut Option<SampleConsumer>)` — sends input mode, oscilloscope params, focus, viewport, audio/vector controls, handles sample rate changes
- `drain_samples(consumer: &mut Option<SampleConsumer>, max: usize) -> Vec<BeamSample>` and `build_sim_frame_info()` — sample draining and frame info construction

### 5. `app.rs` (old) → types split + sim-thread types to simulation.rs

Current `app.rs` types are split:

| Type                | Destination     | Reason                                  |
| ------------------- | --------------- | --------------------------------------- |
| `InputMode`         | `types.rs`      | Shared enum used by UI and sim          |
| `OscilloscopeState` | `types.rs`      | Shared struct cloned between UI and sim |
| `InputState`        | `simulation.rs` | Sim-thread only                         |
| `AudioState`        | `simulation.rs` | Sim-thread only                         |
| `VectorState`       | `simulation.rs` | Sim-thread only                         |
| `ExternalState`     | `simulation.rs` | Sim-thread only                         |
| `ExternalMode`      | `simulation.rs` | Sim-thread only                         |
| `BEAM_ENERGY_SCALE` | `simulation.rs` | Used only by InputState                 |

### 6. UI deduplication — unified `run()` method

`UiState::run()` and `run_detached()` are collapsed. The shared panel body is extracted:

```rust
fn draw_panels(&mut self, ui: &mut egui::Ui, timings, sim_stats, sim_frame)
```

Both code paths call `draw_panels` — the only difference is the container (`SidePanel` vs `CentralPanel`) and which `egui_winit::State` drives input. A single `run()` method takes the `egui_winit::State` as a parameter and a `detached: bool` flag (or enum) to select the container.

### 7. Global shortcut extraction

The three keyboard shortcut chains (Ctrl+Q, Ctrl+D, Ctrl+F) in `window_event()` are collapsed into:

```rust
enum GlobalAction { Quit, ToggleDetach, ToggleFullscreen }
fn check_global_shortcut(event: &WindowEvent, ctx: &egui::Context) -> Option<GlobalAction>
```

## Line Count Estimates (After)

| File                 | Before | After                                   |
| -------------------- | ------ | --------------------------------------- |
| `main.rs`            | 680    | ~15                                     |
| `app.rs`             | 285    | ~300 (App struct + handler)             |
| `controls_window.rs` | —      | ~120                                    |
| `frame.rs`           | —      | ~100                                    |
| `simulation.rs`      | 227    | ~400 (absorbs sim-thread types)         |
| `types.rs`           | 20     | ~60 (adds InputMode, OscilloscopeState) |
| `ui/mod.rs`          | 328    | ~220 (deduplicated)                     |

## What Does NOT Change

- GPU pipeline architecture (gpu/ module)
- Beam sources (beam/ module)
- Phosphor data (phosphor/ module)
- Simulation loop logic
- Threading model (message-passing via crossbeam)
- Any WGSL shaders

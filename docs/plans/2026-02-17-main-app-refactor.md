# Main/App Refactor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Refactor main.rs (680 lines) into focused modules, eliminate duplication between window modes, and clarify type ownership between UI and sim threads.

**Architecture:** Extract-and-focus — same event loop architecture, no shared-state changes. App struct moves to app.rs, ControlsWindow to controls_window.rs, per-frame sync to frame.rs. Shared types go to types.rs, sim-thread types to simulation.rs. UI run/run_detached collapse via shared draw_panels helper.

**Tech Stack:** Rust, winit 0.30, wgpu 27, egui 0.33

---

### Task 1: Move shared types to types.rs

Move types that cross the UI/sim boundary into `types.rs`.

**Files:**

- Modify: `src/types.rs` — add InputMode, OscilloscopeState, ExternalMode, ExternalState
- Modify: `src/app.rs` — remove the moved types (keep InputState, AudioState, VectorState)
- Modify: `src/simulation.rs:9` — update import path
- Modify: `src/ui/mod.rs:10` — update import path
- Modify: `src/ui/scope_panel.rs:3` — update import path
- Modify: `src/presets.rs:1` — update import path

**Step 1: Move type definitions to types.rs**

Cut from `app.rs` and add to `src/types.rs` after the existing `Resolution` type:

- `InputMode` enum (lines 16–23) — needs `strum` derives
- `OscilloscopeState` struct + Default impl (lines 25–44) — needs `crate::presets` import
- `ExternalMode` enum (lines 90–95)
- `ExternalState` struct + Default impl (lines 97–111)

Add required imports to types.rs:

```rust
use crate::beam::oscilloscope::Waveform;
```

Note: `OscilloscopeState::default()` references `crate::presets::OSCILLOSCOPE_PRESETS[0]`, which itself imports `OscilloscopeState`. This creates a circular dependency. Fix by inlining the default values in OscilloscopeState::default() (copy the Circle preset values) and having presets.rs import from types.rs instead.

**Step 2: Update all imports**

```
src/simulation.rs:9  — use crate::app::{InputMode, InputState, OscilloscopeState}
                     → use crate::app::InputState;
                       use crate::types::{InputMode, OscilloscopeState};

src/ui/mod.rs:10     — use crate::app::{ExternalState, InputMode, OscilloscopeState}
                     → use crate::types::{ExternalState, InputMode, OscilloscopeState};

src/ui/scope_panel.rs:3 — use crate::app::{ExternalMode, ExternalState, InputMode, OscilloscopeState}
                        → use crate::types::{ExternalMode, ExternalState, InputMode, OscilloscopeState};

src/presets.rs:1     — use crate::app::OscilloscopeState
                     → use crate::types::OscilloscopeState;

src/app.rs           — update InputState to use crate::types::{InputMode, OscilloscopeState, ExternalState}
                       for its own fields
```

**Step 3: Verify**

Run: `cargo build`
Expected: compiles with no errors

**Step 4: Commit**

```
git add src/types.rs src/app.rs src/simulation.rs src/ui/mod.rs src/ui/scope_panel.rs src/presets.rs
git commit -m "refactor: move shared types to types.rs

Move InputMode, OscilloscopeState, ExternalMode, ExternalState from
app.rs to types.rs. These types cross the UI/sim thread boundary and
belong in the shared types module."
```

---

### Task 2: Move sim-thread types to simulation.rs, delete old app.rs

Move types that are exclusively used by the sim thread into simulation.rs.

**Files:**

- Modify: `src/simulation.rs` — absorb InputState, AudioState, VectorState, BEAM_ENERGY_SCALE
- Delete: `src/app.rs` — all types have been moved out
- Modify: `src/main.rs:3` — remove `mod app;`

**Step 1: Move sim-thread types to simulation.rs**

Cut from `app.rs` and add to `simulation.rs` (before the existing `SimCommand` enum):

- `BEAM_ENERGY_SCALE` constant (line 14)
- `AudioState` struct + Default impl (lines 46–66)
- `VectorState` struct + Default impl (lines 68–88)
- `InputState` struct + Default impl + all impl methods (lines 113–285)

Add required imports to simulation.rs (InputState needs these):

```rust
use crate::beam::audio::AudioSource;
use crate::beam::oscilloscope::{ChannelConfig, OscilloscopeSource, Waveform};
use crate::beam::vector::VectorSegment;
use crate::beam::{BeamSample, BeamSource, BeamState};
use crate::types::{ExternalState, InputMode, OscilloscopeState};
```

Remove the now-redundant `use crate::app::{InputMode, InputState, OscilloscopeState};` from simulation.rs since InputState is local and the others come from types.

**Step 2: Delete app.rs and remove module declaration**

Delete `src/app.rs`. In `src/main.rs`, remove line 3 (`mod app;`).

**Step 3: Verify**

Run: `cargo build`
Expected: compiles with no errors

**Step 4: Commit**

```
git add src/simulation.rs src/main.rs
git rm src/app.rs
git commit -m "refactor: move sim-thread types to simulation.rs

Move InputState, AudioState, VectorState, BEAM_ENERGY_SCALE from app.rs
to simulation.rs where they belong. Delete now-empty app.rs."
```

---

### Task 3: Extract ControlsWindow to controls_window.rs

Extract the detached controls window struct and its rendering from main.rs into a dedicated module.

**Files:**

- Create: `src/controls_window.rs`
- Modify: `src/main.rs` — remove ControlsWindow, add `mod controls_window;`, update references

**Step 1: Create controls_window.rs**

Move from main.rs:

- `ControlsWindow` struct (lines 35–41)
- `ControlsWindow::new()` (lines 43–114)

Add a new `render()` method on ControlsWindow that absorbs the logic from `App::render_controls_window()` (lines 405–503). The method signature:

```rust
pub fn render(
    &mut self,
    gpu: &GpuState,
    ui: &mut UiState,
    sim_stats: Option<&Arc<SimStats>>,
)
```

Extract the egui-to-surface boilerplate (texture upload, encoder, render pass, present) into a helper function:

```rust
fn render_egui_to_surface(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut egui_wgpu::Renderer,
    surface: &wgpu::Surface,
    surface_config: &wgpu::SurfaceConfiguration,
    output: &EguiRenderOutput,
    clear_color: wgpu::Color,
) -> Result<(), wgpu::SurfaceError>
```

This function handles: get_current_texture → update_texture (set) → create encoder → update_buffers → begin_render_pass → render → free_texture → submit → present.

**Step 2: Update main.rs**

- Add `mod controls_window;` to module declarations
- Add `use controls_window::ControlsWindow;`
- Remove the moved struct/impl/method
- In `handle_controls_event` `RedrawRequested` arm, replace `self.render_controls_window(event_loop)` with a call to `controls.render(gpu, ui, sim_stats)` (with appropriate destructuring)
- In `handle_controls_event` `Resized` arm, the surface reconfigure logic stays in App since it accesses both controls and gpu — or move the resize into a method on ControlsWindow

**Step 3: Verify**

Run: `cargo build`
Expected: compiles with no errors

**Step 4: Commit**

```
git add src/controls_window.rs src/main.rs
git commit -m "refactor: extract ControlsWindow to controls_window.rs

Move ControlsWindow struct, constructor, and rendering logic out of
main.rs. Add render_egui_to_surface helper to eliminate boilerplate."
```

---

### Task 4: Deduplicate UI run methods

Collapse `UiState::run()` and `run_detached()` by extracting the shared panel drawing logic.

**Files:**

- Modify: `src/ui/mod.rs` — refactor run/run_detached, add draw_panels helper

**Step 1: Add draw_panels method**

Extract the shared panel body (tab bar + scope_panel/engineer_panel dispatch) into:

```rust
fn draw_panels(
    &mut self,
    ui: &mut egui::Ui,
    fps: f32,
    timings: Option<&TimingHistory>,
    sim_stats: Option<&Arc<SimStats>>,
    sim_frame: Option<&SimFrameInfo>,
)
```

This contains the `ui.horizontal` tab bar + `ui.separator` + the `match self.tab` block — the content that's identical between both methods.

**Step 2: Refactor run() to use draw_panels**

The borrow conflict (`self.ctx.run()` borrows `self.ctx` shared, `draw_panels` needs `&mut self`) is solved by cloning the context first:

```rust
pub fn run(&mut self, window: &Window, ...) -> EguiRenderOutput {
    let raw_input = self.winit_state.take_egui_input(window);
    let ctx = self.ctx.clone(); // Arc-based, cheap clone
    let fps = 1.0 / ctx.input(|i| i.predicted_dt);

    let full_output = ctx.run(raw_input, |egui_ctx| {
        if self.panel_visible {
            let panel_response = egui::SidePanel::left("control_panel")
                .default_width(220.0)
                .show(egui_ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.tab, PanelTab::Scope, "Scope");
                        ui.selectable_value(&mut self.tab, PanelTab::Engineer, "Engineer");
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.small_button("\u{00d7}").clicked() {
                                    self.panel_visible = false;
                                }
                            },
                        );
                    });
                    ui.separator();
                    self.draw_panels(ui, fps, timings, self.accum_size, sim_stats, sim_frame);
                });
            self.panel_width = panel_response.response.rect.width();
        } else {
            self.panel_width = 0.0;
            egui::Area::new(egui::Id::new("panel_toggle"))
                .fixed_pos(egui::pos2(8.0, 8.0))
                .show(egui_ctx, |ui| {
                    if ui.button("\u{2630}").clicked() {
                        self.panel_visible = true;
                    }
                });
        }
    });

    self.winit_state.handle_platform_output(window, full_output.platform_output);
    tessellate_output(&self.ctx, window, full_output)
}
```

**Step 3: Refactor run_detached() to use draw_panels**

```rust
pub fn run_detached(&mut self, window: &Window, egui_winit: &mut egui_winit::State, ...) -> EguiRenderOutput {
    let raw_input = egui_winit.take_egui_input(window);
    let ctx = self.ctx.clone();
    let fps = 1.0 / ctx.input(|i| i.predicted_dt);

    let full_output = ctx.run(raw_input, |egui_ctx| {
        egui::CentralPanel::default().show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, PanelTab::Scope, "Scope");
                ui.selectable_value(&mut self.tab, PanelTab::Engineer, "Engineer");
            });
            ui.separator();
            self.draw_panels(ui, fps, timings, self.accum_size, sim_stats, sim_frame);
        });
    });

    egui_winit.handle_platform_output(window, full_output.platform_output);
    tessellate_output(&self.ctx, window, full_output)
}
```

**Step 4: Extract tessellate_output helper**

The tessellate + ScreenDescriptor construction is identical in both methods:

```rust
fn tessellate_output(
    ctx: &egui::Context,
    window: &Window,
    full_output: egui::FullOutput,
) -> EguiRenderOutput {
    let primitives = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
    let size = window.inner_size();
    EguiRenderOutput {
        primitives,
        textures_delta: full_output.textures_delta,
        screen_descriptor: egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point: full_output.pixels_per_point,
        },
    }
}
```

**Step 5: Verify**

Run: `cargo build`
Expected: compiles with no errors

**Step 6: Commit**

```
git add src/ui/mod.rs
git commit -m "refactor: deduplicate UI run methods

Extract draw_panels() and tessellate_output() helpers to eliminate
code duplication between Combined and Detached window modes."
```

---

### Task 5: Extract frame.rs

Extract per-frame UI→GPU sync and UI→sim command dispatch from main.rs into focused functions.

**Files:**

- Create: `src/frame.rs`
- Modify: `src/main.rs` — replace inline sync/dispatch code with function calls

**Step 1: Create frame.rs with sync_gpu_params**

Extract lines ~217–260 of handle_viewport_event (the GPU parameter copy block):

```rust
use crate::gpu::GpuState;
use crate::types::Resolution;
use crate::ui::UiState;

/// Apply UI state to GPU pipeline parameters. Called once per frame.
pub fn sync_gpu_params(gpu: &mut GpuState, ui: &UiState) {
    let eng = &ui.engineer;
    let scale = eng.accum_resolution_scale;

    // Beam
    gpu.beam_params.sigma_core = ui.focus * scale;
    gpu.beam_params.sigma_halo = eng.sigma_halo * scale;
    gpu.beam_params.halo_fraction = eng.halo_fraction;

    // Faceplate scatter
    gpu.faceplate_scatter_params.threshold = eng.scatter_threshold;
    gpu.faceplate_scatter_params.sigma = eng.scatter_sigma * scale;
    gpu.faceplate_scatter_params.intensity = eng.scatter_intensity;

    // Composite / display
    gpu.composite_params.exposure = ui.intensity;
    gpu.composite_params.set_mode(eng.tonemap_mode);
    gpu.composite_params.faceplate_scatter_intensity = eng.scatter_intensity;
    gpu.composite_params.glass_tint = eng.glass_tint;
    gpu.composite_params.curvature = eng.curvature;
    gpu.composite_params.edge_falloff = eng.edge_falloff;

    // Accumulation buffer resize if resolution scale changed
    let target = Resolution::new(
        ((gpu.surface_config.width as f32) * scale).round().max(1.0) as u32,
        ((gpu.surface_config.height as f32) * scale).round().max(1.0) as u32,
    );
    if target != gpu.accum.resolution {
        gpu.resize_buffers(target);
    }
}
```

**Step 2: Add dispatch_sim_commands to frame.rs**

Extract lines ~309–344 of handle_viewport_event (the SimCommand sending block):

```rust
use crate::beam::SampleConsumer;
use crate::simulation::SimCommand;

/// Send current UI state to the simulation thread. Called once per frame.
pub fn dispatch_sim_commands(
    tx: &crossbeam_channel::Sender<SimCommand>,
    ui: &mut UiState,
    gpu: &GpuState,
    sidebar_width: f32,
    sample_rate: &mut f32,
    sim_consumer: &mut Option<SampleConsumer>,
) {
    let _ = tx.send(SimCommand::SetInputMode(ui.input_mode));
    let _ = tx.send(SimCommand::SetOscilloscopeParams(ui.oscilloscope.clone()));
    let _ = tx.send(SimCommand::SetFocus(ui.focus));
    let _ = tx.send(SimCommand::SetViewport {
        width: gpu.surface_config.width as f32 - sidebar_width,
        height: gpu.surface_config.height as f32,
        x_offset: sidebar_width,
    });

    // Audio controls
    let _ = tx.send(SimCommand::SetAudioPlaying(ui.audio_ui.playing));
    let _ = tx.send(SimCommand::SetAudioLooping(ui.audio_ui.looping));
    let _ = tx.send(SimCommand::SetAudioSpeed(ui.audio_ui.speed));
    if let Some(path) = ui.audio_ui.pending_file.take() {
        ui.audio_ui.file_path = Some(path.clone());
        ui.audio_ui.has_file = true;
        let _ = tx.send(SimCommand::LoadAudioFile(path));
    }

    // Vector controls
    if let Some(path) = ui.vector_ui.pending_file.take() {
        ui.vector_ui.file_path = Some(path.clone());
        let _ = tx.send(SimCommand::LoadVectorFile(path));
    }

    // Sample rate change — recreate the ring buffer
    if ui.oscilloscope.sample_rate != *sample_rate {
        *sample_rate = ui.oscilloscope.sample_rate;
        let capacity = (*sample_rate as usize * 3 / 2).next_power_of_two();
        let (producer, consumer) = crate::beam::sample_channel(capacity);
        *sim_consumer = Some(consumer);
        let _ = tx.send(SimCommand::SetSampleRate {
            rate: *sample_rate,
            producer,
        });
    }
}
```

**Step 3: Update main.rs**

- Add `mod frame;` to module declarations
- Replace the inline sync block in handle_viewport_event with `frame::sync_gpu_params(gpu, ui);`
- Replace the inline dispatch block with `frame::dispatch_sim_commands(tx, ui, gpu, sidebar_width, &mut self.sample_rate, &mut self.sim_consumer);`
- The viewport offset / viewport size computation (`gpu.composite_params.viewport_offset`, `viewport_size`) stays near the dispatch call in handle_viewport_event since it depends on sidebar_width

**Step 4: Verify**

Run: `cargo build`
Expected: compiles with no errors

**Step 5: Commit**

```
git add src/frame.rs src/main.rs
git commit -m "refactor: extract per-frame sync to frame.rs

Move UI→GPU parameter sync and UI→sim command dispatch into focused
functions in frame.rs, slimming handle_viewport_event by ~60 lines."
```

---

### Task 6: Move App to app.rs + extract shortcuts + slim main.rs

Move the App struct and ApplicationHandler impl from main.rs into a new app.rs. Extract keyboard shortcut handling. Slim main.rs to entry point only.

**Files:**

- Create: `src/app.rs` (new file, old one was deleted in task 2)
- Modify: `src/main.rs` — slim to entry point

**Step 1: Extract global shortcut handler**

Create a helper in app.rs (or as a method on App):

```rust
enum GlobalAction {
    Quit,
    ToggleDetach,
    ToggleFullscreen,
}

fn check_global_shortcut(
    event: &WindowEvent,
    ctx: &egui::Context,
) -> Option<GlobalAction> {
    let WindowEvent::KeyboardInput {
        event: winit::event::KeyEvent {
            physical_key: winit::keyboard::PhysicalKey::Code(key_code),
            state: winit::event::ElementState::Pressed,
            ..
        },
        ..
    } = event else {
        return None;
    };

    let has_modifier = ctx.input(|i| i.modifiers.ctrl || i.modifiers.mac_cmd);
    if !has_modifier {
        return None;
    }

    match key_code {
        winit::keyboard::KeyCode::KeyQ => Some(GlobalAction::Quit),
        winit::keyboard::KeyCode::KeyD => Some(GlobalAction::ToggleDetach),
        winit::keyboard::KeyCode::KeyF => Some(GlobalAction::ToggleFullscreen),
        _ => None,
    }
}
```

**Step 2: Move App to app.rs**

Move from main.rs to the new app.rs:

- `WindowMode` enum
- `App` struct + `Default` impl
- `App` methods: `toggle_detach`, `handle_viewport_event`, `handle_controls_event`
- `ApplicationHandler` impl (`resumed`, `window_event`, `about_to_wait`)
- `DEFAULT_FRAME_INTERVAL` constant
- `GlobalAction` enum + `check_global_shortcut` function

The `window_event` method is rewritten to use `check_global_shortcut`:

```rust
fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
    if let Some(ui) = &self.ui
        && let Some(action) = check_global_shortcut(&event, &ui.ctx)
    {
        match action {
            GlobalAction::Quit => event_loop.exit(),
            GlobalAction::ToggleDetach => self.toggle_detach(event_loop),
            GlobalAction::ToggleFullscreen => {
                if let Some(window) = &self.window {
                    if window.fullscreen().is_some() {
                        window.set_fullscreen(None);
                    } else {
                        window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                    }
                }
            }
        }
        return;
    }

    // Route by window ID
    let is_viewport = self.window.as_ref().is_some_and(|w| w.id() == window_id);
    let is_controls = self.controls.as_ref().is_some_and(|c| c.window.id() == window_id);

    if is_controls {
        self.handle_controls_event(event_loop, event);
    } else if is_viewport {
        self.handle_viewport_event(event_loop, event);
    }
}
```

Add required imports to app.rs: `use crate::{frame, gpu, ui, controls_window, simulation, ...}` etc.

**Step 3: Slim main.rs to entry point**

main.rs becomes:

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
    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::stderr());
    let env_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive("phosphor=info".parse()?)
        .from_env()?;
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(non_blocking)
        .init();

    let event_loop = winit::event_loop::EventLoop::new().expect("failed to create event loop");
    let mut app = app::App::default();
    event_loop.run_app(&mut app).expect("event loop error");

    Ok(())
}
```

**Step 4: Verify**

Run: `cargo build`
Expected: compiles with no errors

**Step 5: Smoke test**

Run: `cargo run`

- Verify CRT viewport renders correctly
- Verify detached controls window opens
- Verify Ctrl+D toggles between modes
- Verify Ctrl+F toggles fullscreen
- Verify Ctrl+Q quits

**Step 6: Commit**

```
git add src/app.rs src/main.rs
git commit -m "refactor: move App struct to app.rs, slim main.rs to entry point

Move App, WindowMode, ApplicationHandler impl, and shortcut handling
to app.rs. Extract check_global_shortcut() to replace three duplicated
keyboard match chains. main.rs is now ~20 lines."
```

---

### Task 7: Update CLAUDE.md module structure

Update the module structure section in CLAUDE.md to reflect the new file layout.

**Files:**

- Modify: `CLAUDE.md` — update Module Structure section

**Step 1: Update module structure**

Update the `src/` tree in CLAUDE.md to show:

```
src/
  main.rs              — entry point, tracing init, event loop
  app.rs               — App struct, ApplicationHandler, WindowMode, shortcut handling
  controls_window.rs   — ControlsWindow struct, detached controls rendering
  frame.rs             — per-frame UI→GPU sync, UI→sim dispatch
  simulation.rs        — SimCommand, InputState, AudioState, VectorState, sim loop
  ...
```

Remove `app.rs — InputState, per-mode state` from the old listing.

**Step 2: Commit**

```
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md module structure for refactor"
```

---

### Task 8: Clean up plan files

Delete the design doc and implementation plan from docs/plans/.

**Step 1: Remove plan files**

```
git rm docs/plans/2026-02-17-main-app-refactor-design.md
git rm docs/plans/2026-02-17-main-app-refactor.md
git commit -m "chore: remove completed refactor plan files"
```

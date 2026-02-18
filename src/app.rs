use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{Window, WindowId};

use crate::beam::SampleConsumer;
use crate::controls_window::ControlsWindow;
use crate::gpu::GpuState;
use crate::simulation::SimCommand;
use crate::simulation_stats::SimStats;
use crate::ui::{SimFrameInfo, UiState};

#[derive(Default, PartialEq)]
enum WindowMode {
    Combined,
    #[default]
    Detached,
}

enum GlobalAction {
    Quit,
    ToggleDetach,
    ToggleFullscreen,
}

fn check_global_shortcut(event: &WindowEvent, ctx: &egui::Context) -> Option<GlobalAction> {
    let WindowEvent::KeyboardInput {
        event:
            winit::event::KeyEvent {
                physical_key: winit::keyboard::PhysicalKey::Code(key_code),
                state: winit::event::ElementState::Pressed,
                ..
            },
        ..
    } = event
    else {
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

/// Fallback frame interval when the monitor refresh rate can't be queried.
const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_micros(16_667); // 60 Hz

pub struct App {
    // Drop order matters: GPU resources (surfaces) must be dropped before the
    // windows they reference, so `gpu` and `controls` are declared before `window`.
    gpu: Option<GpuState>,
    controls: Option<ControlsWindow>,
    ui: Option<UiState>,
    mode: WindowMode,
    window: Option<Arc<Window>>,
    frame_interval: Duration,
    next_frame: Instant,
    // Simulation thread
    sim_consumer: Option<SampleConsumer>,
    sim_commands: Option<crossbeam_channel::Sender<SimCommand>>,
    sim_handle: Option<std::thread::JoinHandle<()>>,
    sim_stats: Option<Arc<SimStats>>,
    sample_rate: f32,
}

impl Default for App {
    fn default() -> Self {
        Self {
            gpu: None,
            controls: None,
            ui: None,
            mode: WindowMode::default(),
            window: None,
            frame_interval: DEFAULT_FRAME_INTERVAL,
            next_frame: Instant::now(),
            sim_consumer: None,
            sim_commands: None,
            sim_handle: None,
            sim_stats: None,
            sample_rate: 44100.0,
        }
    }
}

impl App {
    fn toggle_detach(&mut self, event_loop: &ActiveEventLoop) {
        match self.mode {
            WindowMode::Combined => {
                let Some(gpu) = &self.gpu else { return };
                let Some(ui) = &self.ui else { return };
                if let Some(controls) = ControlsWindow::new(event_loop, gpu, ui.ctx.clone()) {
                    self.controls = Some(controls);
                    self.mode = WindowMode::Detached;
                    tracing::info!("Detached controls to separate window");
                }
            }
            WindowMode::Detached => {
                self.controls = None;
                self.mode = WindowMode::Combined;
                if let Some(ui) = &mut self.ui {
                    ui.panel_visible = true;
                }
                tracing::info!("Combined controls back into main window");
            }
        }
    }

    fn handle_viewport_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        // Only pass events to egui in Combined mode (viewport shouldn't
        // consume events for an invisible panel in Detached mode)
        if self.mode == WindowMode::Combined
            && let Some(ui) = &mut self.ui
            && let Some(window) = &self.window
        {
            let response = ui.on_event(window, &event);
            if response.consumed {
                return;
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                if let Some(tx) = self.sim_commands.take() {
                    let _ = tx.send(SimCommand::Shutdown);
                }
                if let Some(handle) = self.sim_handle.take() {
                    let _ = handle.join();
                }
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let scale = self
                    .ui
                    .as_ref()
                    .map_or(1.0, |ui| ui.engineer.accum_resolution_scale);
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height, scale);
                }
            }
            WindowEvent::RedrawRequested => {
                let Some(window) = &self.window else { return };
                let Some(gpu) = &mut self.gpu else { return };
                let Some(ui) = &mut self.ui else { return };

                // Phosphor change: rebuild decay/emission/spectral params + buffer
                if ui.phosphor_changed() {
                    gpu.switch_phosphor(ui.selected_phosphor());
                }

                // Apply UI state to GPU parameters
                crate::frame::sync_gpu_params(gpu, ui);

                // Feed accumulation buffer size to UI for display
                ui.accum_size = Some(gpu.accum.resolution);

                // Drain samples from simulation thread's ring buffer.
                // Cap at 2x frame interval to prevent catastrophic decay during stalls.
                let max_dt = self.frame_interval.as_secs_f32() * 2.0;
                let max_samples = (self.sample_rate * max_dt) as usize;
                let samples = self
                    .sim_consumer
                    .as_mut()
                    .map(|c| c.drain_up_to(max_samples))
                    .unwrap_or_default();
                let sim_dt = if samples.is_empty() {
                    0.0
                } else {
                    samples.len() as f32 / self.sample_rate
                };

                // Build per-frame simulation info for the engineer panel
                let sim_frame_info = SimFrameInfo {
                    samples_this_frame: samples.len(),
                    sim_dt,
                    buffer_pending: self.sim_consumer.as_ref().map_or(0, |c| c.pending()),
                };

                // Run egui frame only in Combined mode
                let egui_output = if self.mode == WindowMode::Combined {
                    let timings = gpu.profiler.as_ref().map(|p| &p.history);
                    Some(ui.run(
                        window,
                        timings,
                        self.sim_stats.as_ref(),
                        Some(&sim_frame_info),
                    ))
                } else {
                    None
                };

                // Forward UI state changes to the simulation thread
                let sidebar_width = if self.mode == WindowMode::Combined {
                    ui.panel_width
                } else {
                    0.0
                };
                gpu.composite_params.viewport_offset = [sidebar_width, 0.0];
                gpu.composite_params.viewport_size = [
                    gpu.surface_config.width as f32 - sidebar_width,
                    gpu.surface_config.height as f32,
                ];

                if let Some(tx) = &self.sim_commands {
                    crate::frame::dispatch_sim_commands(
                        tx,
                        ui,
                        gpu,
                        sidebar_width,
                        &mut self.sample_rate,
                        &mut self.sim_consumer,
                    );
                }

                match gpu.render(&samples, sim_dt, egui_output.as_ref()) {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost) => {
                        let (w, h) = (gpu.surface_config.width, gpu.surface_config.height);
                        gpu.resize(w, h, ui.engineer.accum_resolution_scale);
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        tracing::error!("GPU out of memory");
                        event_loop.exit();
                    }
                    Err(e) => {
                        tracing::warn!("Surface error: {e:?}");
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_controls_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        // Pass events to controls egui_winit
        if let Some(controls) = &mut self.controls {
            let _response = controls
                .egui_winit
                .on_window_event(&controls.window, &event);
        }

        match event {
            WindowEvent::CloseRequested => {
                // Recombine: drop controls, go back to Combined mode
                self.controls = None;
                self.mode = WindowMode::Combined;
                if let Some(ui) = &mut self.ui {
                    ui.panel_visible = true;
                }
                tracing::info!("Controls window closed, recombined into main window");
            }
            WindowEvent::Resized(size) => {
                if let Some(controls) = &mut self.controls
                    && size.width > 0
                    && size.height > 0
                {
                    controls.surface_config.width = size.width;
                    controls.surface_config.height = size.height;
                    if let Some(gpu) = &self.gpu {
                        controls
                            .surface
                            .configure(&gpu.device, &controls.surface_config);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let (controls, gpu, ui) = match (&mut self.controls, &self.gpu, &mut self.ui) {
                    (Some(c), Some(g), Some(u)) => (c, g, u),
                    _ => return,
                };
                match controls.render(gpu, ui, self.sim_stats.as_ref()) {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost) => {
                        controls
                            .surface
                            .configure(&gpu.device, &controls.surface_config);
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        tracing::error!("GPU out of memory (controls window)");
                        event_loop.exit();
                    }
                    Err(e) => {
                        tracing::warn!("Controls surface error: {e:?}");
                    }
                }
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes().with_title("Phosphor");

        let window: Arc<Window> = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        // Query the monitor's native refresh rate for frame pacing.
        // Fifo present mode alone isn't reliable on all Linux compositors,
        // so we also pace via ControlFlow::WaitUntil in about_to_wait.
        if let Some(monitor) = window.current_monitor()
            && let Some(millihertz) = monitor.refresh_rate_millihertz()
        {
            let micros = 1_000_000_000 / millihertz as u64;
            self.frame_interval = Duration::from_micros(micros);
            tracing::info!(
                "Monitor refresh rate: {:.1} Hz (frame interval: {:.2} ms)",
                millihertz as f64 / 1000.0,
                micros as f64 / 1000.0,
            );
        }

        let mut gpu = GpuState::new(window.clone());
        let ui = UiState::new(&window);
        gpu.switch_phosphor(ui.selected_phosphor());

        // Spawn simulation thread
        let buffer_capacity = 65536;
        let (producer, consumer) = crate::beam::sample_channel(buffer_capacity);
        let stats = SimStats::new(buffer_capacity as u32);
        let (handle, cmd_tx) = crate::simulation::spawn_simulation(producer, stats.clone());

        // Send initial viewport dimensions
        let size = window.inner_size();
        let _ = cmd_tx.send(SimCommand::SetViewport {
            width: size.width as f32,
            height: size.height as f32,
            x_offset: 0.0,
        });

        // If starting in detached mode, create the controls window immediately
        if self.mode == WindowMode::Detached
            && let Some(controls) = ControlsWindow::new(event_loop, &gpu, ui.ctx.clone())
        {
            self.controls = Some(controls);
        }

        self.sim_consumer = Some(consumer);
        self.sim_commands = Some(cmd_tx);
        self.sim_handle = Some(handle);
        self.sim_stats = Some(stats);
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.ui = Some(ui);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
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
                            window
                                .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                        }
                    }
                }
            }
            return;
        }

        // Route by window ID
        let is_viewport = self.window.as_ref().is_some_and(|w| w.id() == window_id);
        let is_controls = self
            .controls
            .as_ref()
            .is_some_and(|c| c.window.id() == window_id);

        if is_controls {
            self.handle_controls_event(event_loop, event);
        } else if is_viewport {
            self.handle_viewport_event(event_loop, event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        if let Some(controls) = &self.controls {
            controls.window.request_redraw();
        }
        // Pace frames to the monitor's native refresh rate. Fifo present
        // mode should do this via swapchain blocking, but doesn't reliably
        // engage on all Linux Vulkan compositors.
        self.next_frame += self.frame_interval;
        // If we fell behind (e.g. long frame), reset to avoid a burst of catch-up frames
        let now = Instant::now();
        if self.next_frame < now {
            self.next_frame = now + self.frame_interval;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }
}

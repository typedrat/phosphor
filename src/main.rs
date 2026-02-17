#![allow(dead_code, unused_imports)]

mod app;
mod beam;
mod gpu;
mod phosphor;
mod simulation;
mod simulation_stats;
mod types;
mod ui;

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use beam::SampleConsumer;
use gpu::GpuState;
use simulation::SimCommand;
use simulation_stats::SimStats;
use types::Resolution;
use ui::{SimFrameInfo, UiState};

#[derive(Default, PartialEq)]
enum WindowMode {
    #[default]
    Combined,
    Detached,
}

struct ControlsWindow {
    egui_renderer: egui_wgpu::Renderer,
    egui_winit: egui_winit::State,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    window: Arc<Window>,
}

impl ControlsWindow {
    fn new(event_loop: &ActiveEventLoop, gpu: &GpuState, egui_ctx: egui::Context) -> Option<Self> {
        let attrs = Window::default_attributes()
            .with_title("Phosphor \u{2014} Controls")
            .with_inner_size(winit::dpi::LogicalSize::new(320.0, 600.0));

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create controls window: {e}");
                return None;
            }
        };

        let surface = gpu.instance.create_surface(window.clone()).ok()?;
        let size = window.inner_size();

        let surface_caps = surface.get_capabilities(&gpu.adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&gpu.device, &surface_config);

        let mut egui_renderer = egui_wgpu::Renderer::new(&gpu.device, format, Default::default());

        // The shared egui::Context already has a font atlas loaded (uploaded to
        // the viewport's renderer). This new renderer needs its own copy.
        // Font atlas is always TextureId::Managed(0).
        let font_delta = egui_ctx.fonts(|fonts| {
            egui::epaint::ImageDelta::full(
                egui::epaint::ImageData::Color(std::sync::Arc::new(fonts.image())),
                egui::TextureOptions::LINEAR,
            )
        });
        egui_renderer.update_texture(
            &gpu.device,
            &gpu.queue,
            egui::TextureId::Managed(0),
            &font_delta,
        );

        let egui_winit = egui_winit::State::new(
            egui_ctx,
            egui::ViewportId::from_hash_of("controls"),
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );

        Some(Self {
            window,
            surface,
            surface_config,
            egui_renderer,
            egui_winit,
        })
    }
}

/// Fallback frame interval when the monitor refresh rate can't be queried.
const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_micros(16_667); // 60 Hz

struct App {
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
                let eng = &ui.engineer;

                // Beam -- scope focus overrides core sigma, engineer controls the rest
                gpu.beam_params.sigma_core = ui.focus;
                gpu.beam_params.sigma_halo = eng.sigma_halo;
                gpu.beam_params.halo_fraction = eng.halo_fraction;

                // Faceplate scatter
                gpu.faceplate_scatter_params.threshold = eng.scatter_threshold;
                gpu.faceplate_scatter_params.sigma = eng.scatter_sigma;
                gpu.faceplate_scatter_params.intensity = eng.scatter_intensity;

                // Accumulation buffer resolution scale
                let scale = eng.accum_resolution_scale;
                let target = Resolution::new(
                    ((gpu.surface_config.width as f32) * scale).round().max(1.0) as u32,
                    ((gpu.surface_config.height as f32) * scale)
                        .round()
                        .max(1.0) as u32,
                );
                if target != gpu.accum.resolution {
                    gpu.resize_buffers(target);
                }

                // Feed accumulation buffer size to UI for display
                ui.accum_size = Some(gpu.accum.resolution);

                // Composite / display
                gpu.composite_params.exposure = ui.intensity;
                gpu.composite_params.set_mode(eng.tonemap_mode);
                gpu.composite_params.faceplate_scatter_intensity = eng.scatter_intensity;
                gpu.composite_params.glass_tint = eng.glass_tint;
                gpu.composite_params.curvature = eng.curvature;
                gpu.composite_params.edge_falloff = eng.edge_falloff;

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
                    let _ = tx.send(SimCommand::SetInputMode(ui.input_mode));
                    let _ = tx.send(SimCommand::SetOscilloscopeParams(ui.oscilloscope.clone()));
                    let _ = tx.send(SimCommand::SetFocus(ui.focus));
                    let _ = tx.send(SimCommand::SetViewport {
                        width: gpu.surface_config.width as f32 - sidebar_width,
                        height: gpu.surface_config.height as f32,
                        x_offset: sidebar_width,
                    });
                    let _ = tx.send(SimCommand::SetAccumResolution(gpu.accum.resolution));

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
                self.render_controls_window(event_loop);
            }
            _ => {}
        }
    }

    fn render_controls_window(&mut self, event_loop: &ActiveEventLoop) {
        let (controls, gpu, ui) = match (&mut self.controls, &self.gpu, &mut self.ui) {
            (Some(c), Some(g), Some(u)) => (c, g, u),
            _ => return,
        };

        // Get surface texture
        let output = match controls.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost) => {
                controls
                    .surface
                    .configure(&gpu.device, &controls.surface_config);
                return;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                tracing::error!("GPU out of memory (controls window)");
                event_loop.exit();
                return;
            }
            Err(e) => {
                tracing::warn!("Controls surface error: {e:?}");
                return;
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Run egui in detached mode
        let timings = gpu.profiler.as_ref().map(|p| &p.history);
        let egui_output = ui.run_detached(
            &controls.window,
            &mut controls.egui_winit,
            timings,
            self.sim_stats.as_ref(),
            None, // sim_frame only available during viewport redraw
        );

        // Update egui textures
        for (id, delta) in &egui_output.textures_delta.set {
            controls
                .egui_renderer
                .update_texture(&gpu.device, &gpu.queue, *id, delta);
        }

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("controls_frame"),
            });

        controls.egui_renderer.update_buffers(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &egui_output.primitives,
            &egui_output.screen_descriptor,
        );

        // Clear + render egui
        {
            let mut rpass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("controls_egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.1,
                                g: 0.1,
                                b: 0.1,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    ..Default::default()
                })
                .forget_lifetime();

            controls.egui_renderer.render(
                &mut rpass,
                &egui_output.primitives,
                &egui_output.screen_descriptor,
            );
        }

        for id in &egui_output.textures_delta.free {
            controls.egui_renderer.free_texture(id);
        }

        gpu.queue.submit(std::iter::once(encoder.finish()));
        output.present();
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
        // Intercept Ctrl/Cmd+Q to quit
        if let WindowEvent::KeyboardInput {
            event:
                winit::event::KeyEvent {
                    physical_key: winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyQ),
                    state: winit::event::ElementState::Pressed,
                    ..
                },
            ..
        } = &event
            && let Some(ui) = &self.ui
            && ui.ctx.input(|i| i.modifiers.ctrl || i.modifiers.mac_cmd)
        {
            event_loop.exit();
            return;
        }

        // Intercept Ctrl+D for detach/attach toggle (on any window)
        if let WindowEvent::KeyboardInput {
            event:
                winit::event::KeyEvent {
                    physical_key: winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyD),
                    state: winit::event::ElementState::Pressed,
                    ..
                },
            ..
        } = &event
            && let Some(ui) = &self.ui
            && ui.ctx.input(|i| i.modifiers.ctrl || i.modifiers.mac_cmd)
        {
            self.toggle_detach(event_loop);
            return;
        }

        // Intercept Ctrl/Cmd+F for fullscreen toggle on viewport window
        if let WindowEvent::KeyboardInput {
            event:
                winit::event::KeyEvent {
                    physical_key: winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyF),
                    state: winit::event::ElementState::Pressed,
                    ..
                },
            ..
        } = &event
            && let Some(ui) = &self.ui
            && ui.ctx.input(|i| i.modifiers.ctrl || i.modifiers.mac_cmd)
        {
            if let Some(window) = &self.window {
                if window.fullscreen().is_some() {
                    window.set_fullscreen(None);
                } else {
                    window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
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

fn main() -> anyhow::Result<()> {
    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::stderr());
    let env_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive("phosphor=info".parse()?)
        .from_env()?;
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(non_blocking)
        .init();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop error");

    Ok(())
}

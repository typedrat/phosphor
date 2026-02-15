mod beam;
mod gpu;
mod phosphor;
mod ui;

use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use gpu::GpuState;
use ui::UiState;

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    ui: Option<UiState>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            window: None,
            gpu: None,
            ui: None,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes().with_title("Phosphor");

        let window: Arc<Window> = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("Failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let gpu = GpuState::new(window.clone());
        let ui = UiState::new(&window);
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.ui = Some(ui);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Pass events to egui first
        if let Some(ui) = &mut self.ui {
            if let Some(window) = &self.window {
                let response = ui.on_event(&window, &event);
                if response.consumed {
                    return;
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                let Some(window) = &self.window else { return };
                let Some(gpu) = &mut self.gpu else { return };
                let Some(ui) = &mut self.ui else { return };

                // Apply UI state to GPU parameters
                let phosphor = ui.selected_phosphor();
                gpu.beam_params.sigma_core = ui.focus;
                gpu.decay_params = gpu::decay::DecayParams::new(
                    phosphor.fluorescence.tau_fast,
                    phosphor.fluorescence.tau_slow,
                );
                gpu.emission_params = gpu::beam_write::EmissionParams::new(
                    &phosphor.fluorescence.emission_weights,
                    phosphor.fluorescence.a_fast,
                );
                gpu.composite_params.exposure = ui.intensity;
                gpu.composite_params.faceplate_scatter_intensity = ui.faceplate_scatter_intensity;
                gpu.composite_params.glass_tint = ui.glass_tint;
                gpu.composite_params.curvature = ui.curvature;
                gpu.composite_params.edge_falloff = ui.edge_falloff;

                // Run egui frame
                let egui_output = ui.run(&window);

                match gpu.render(&[], Some(&egui_output)) {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost) => {
                        let (w, h) = (gpu.surface_config.width, gpu.surface_config.height);
                        gpu.resize(w, h);
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        log::error!("GPU out of memory");
                        event_loop.exit();
                    }
                    Err(e) => {
                        log::warn!("Surface error: {e:?}");
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop error");
}

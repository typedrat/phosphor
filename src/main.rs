mod beam;
mod gpu;
mod phosphor;

use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

use gpu::GpuState;

struct App {
    window: Option<Arc<dyn Window>>,
    gpu: Option<GpuState>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            window: None,
            gpu: None,
        }
    }
}

impl ApplicationHandler for App {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        let attrs = WindowAttributes::default().with_title("Phosphor");

        let window: Arc<dyn Window> = match event_loop.create_window(attrs) {
            Ok(w) => w.into(),
            Err(e) => {
                log::error!("Failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let gpu = GpuState::new(window.clone());
        self.window = Some(window);
        self.gpu = Some(gpu);
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::SurfaceResized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                let Some(gpu) = &mut self.gpu else { return };
                match gpu.render() {
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

    fn about_to_wait(&mut self, _event_loop: &dyn ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop
        .run_app(App::default())
        .expect("event loop error");
}

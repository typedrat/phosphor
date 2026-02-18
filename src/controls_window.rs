use std::sync::Arc;

use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::gpu::GpuState;
use crate::simulation_stats::SimStats;
use crate::ui::{EguiRenderOutput, UiState};

pub struct ControlsWindow {
    egui_renderer: egui_wgpu::Renderer,
    pub egui_winit: egui_winit::State,
    pub surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
    pub window: Arc<Window>,
}

impl ControlsWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        gpu: &GpuState,
        egui_ctx: egui::Context,
    ) -> Option<Self> {
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

    pub fn render(
        &mut self,
        gpu: &GpuState,
        ui: &mut UiState,
        sim_stats: Option<&Arc<SimStats>>,
    ) -> Result<(), wgpu::SurfaceError> {
        let timings = gpu.profiler.as_ref().map(|p| &p.history);
        let egui_output = ui.run_detached(
            &self.window,
            &mut self.egui_winit,
            timings,
            sim_stats,
            None, // sim_frame only available during viewport redraw
        );

        render_egui_to_surface(
            &gpu.device,
            &gpu.queue,
            &mut self.egui_renderer,
            &self.surface,
            &egui_output,
            wgpu::Color {
                r: 0.1,
                g: 0.1,
                b: 0.1,
                a: 1.0,
            },
        )
    }
}

fn render_egui_to_surface(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut egui_wgpu::Renderer,
    surface: &wgpu::Surface,
    output: &EguiRenderOutput,
    clear_color: wgpu::Color,
) -> Result<(), wgpu::SurfaceError> {
    let surface_texture = surface.get_current_texture()?;
    let view = surface_texture
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    for (id, delta) in &output.textures_delta.set {
        renderer.update_texture(device, queue, *id, delta);
    }

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("controls_frame"),
    });

    renderer.update_buffers(
        device,
        queue,
        &mut encoder,
        &output.primitives,
        &output.screen_descriptor,
    );

    {
        let mut rpass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("controls_egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            })
            .forget_lifetime();

        renderer.render(&mut rpass, &output.primitives, &output.screen_descriptor);
    }

    for id in &output.textures_delta.free {
        renderer.free_texture(id);
    }

    queue.submit(std::iter::once(encoder.finish()));
    surface_texture.present();

    Ok(())
}

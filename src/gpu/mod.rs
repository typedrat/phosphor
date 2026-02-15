pub mod accumulation;
pub mod beam_write;
pub mod composite;
pub mod decay;
pub mod spectral_resolve;

use std::sync::Arc;
use std::time::Instant;

use phosphor_data::spectral::SPECTRAL_BANDS;

use winit::window::Window;

use crate::beam::BeamSample;
use crate::ui::EguiRenderOutput;

const SPECTRAL_CONSTANTS: &[(&str, f64)] = &[("SPECTRAL_BANDS", SPECTRAL_BANDS as f64)];

use self::accumulation::{AccumulationBuffer, HdrBuffer};
use self::beam_write::{BeamParams, BeamWritePipeline, EmissionParams};
use self::composite::{CompositeParams, CompositePipeline, TonemapMode};
use self::decay::{DecayParams, DecayPipeline};
use self::spectral_resolve::{SpectralResolveParams, SpectralResolvePipeline};

pub struct GpuState {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
    pub accum: AccumulationBuffer,
    pub hdr: HdrBuffer,
    pub beam_write: BeamWritePipeline,
    pub beam_params: BeamParams,
    pub emission_params: EmissionParams,
    pub decay: DecayPipeline,
    pub decay_params: DecayParams,
    pub spectral_resolve: SpectralResolvePipeline,
    pub spectral_resolve_params: SpectralResolveParams,
    pub composite: CompositePipeline,
    pub composite_params: CompositeParams,
    pub egui_renderer: egui_wgpu::Renderer,
    /// Whether the swapchain surface supports HDR output.
    pub hdr_output: bool,
    last_frame: Instant,
}

impl GpuState {
    pub fn new(window: Arc<Window>) -> Self {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("no suitable GPU adapter found");

        log::info!("GPU adapter: {}", adapter.get_info().name);

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("phosphor"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .expect("failed to create GPU device");

        let size = window.inner_size();
        let surface_caps = surface.get_capabilities(&adapter);

        // Prefer HDR surface format (Rgba16Float) when the display supports it,
        // falling back to sRGB for standard displays.
        let hdr_format = surface_caps
            .formats
            .iter()
            .find(|f| **f == wgpu::TextureFormat::Rgba16Float)
            .copied();
        let (format, hdr_output) = if let Some(fmt) = hdr_format {
            log::info!("HDR surface format available: {fmt:?}");
            (fmt, true)
        } else {
            let srgb = surface_caps
                .formats
                .iter()
                .find(|f| f.is_srgb())
                .copied()
                .unwrap_or(surface_caps.formats[0]);
            log::info!("SDR surface format: {srgb:?}");
            (srgb, false)
        };

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
        surface.configure(&device, &surface_config);

        // Single layer for now (8 textures: 4 fast + 4 slow)
        let accum =
            AccumulationBuffer::new(&device, surface_config.width, surface_config.height, 1);

        let beam_write = BeamWritePipeline::new(&device);

        // Default beam parameters — will be configurable via UI later
        let beam_params = BeamParams::new(
            1.5,  // sigma_core (pixels)
            6.0,  // sigma_halo (pixels)
            0.03, // halo_fraction
            surface_config.width,
            surface_config.height,
        );

        // Default P1 green phosphor emission — uniform across bands for now
        let emission_params = EmissionParams::new(&[1.0 / 16.0; 16], 0.7);

        let decay = DecayPipeline::new(&device);

        // Default P1 green phosphor decay — ~12ms fast, ~40ms slow
        let decay_params = DecayParams::new(0.012, 0.040);

        let hdr = HdrBuffer::new(&device, surface_config.width, surface_config.height);

        let spectral_resolve = SpectralResolvePipeline::new(&device);
        let spectral_resolve_params = SpectralResolveParams::new();

        let composite = CompositePipeline::new(&device, format);
        let tonemap_mode = if hdr_output {
            TonemapMode::None
        } else {
            TonemapMode::default()
        };
        let composite_params = CompositeParams::new(1.0, tonemap_mode);

        let egui_renderer = egui_wgpu::Renderer::new(&device, format, Default::default());

        Self {
            device,
            queue,
            surface,
            surface_config,
            accum,
            hdr,
            beam_write,
            beam_params,
            emission_params,
            decay,
            decay_params,
            spectral_resolve,
            spectral_resolve_params,
            composite,
            composite_params,
            egui_renderer,
            hdr_output,
            last_frame: Instant::now(),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
            self.accum.resize(&self.device, width, height);
            self.hdr.resize(&self.device, width, height);
            self.beam_params.width = width;
            self.beam_params.height = height;
        }
    }

    pub fn render(
        &mut self,
        samples: &[BeamSample],
        egui: Option<&EguiRenderOutput>,
    ) -> Result<(), wgpu::SurfaceError> {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });

        // Beam write pass
        if !samples.is_empty() {
            let params = self.beam_params.with_sample_count(samples.len() as u32);
            self.beam_write.dispatch(
                &self.device,
                &self.queue,
                &mut encoder,
                samples,
                &params,
                &self.emission_params,
                &self.accum,
            );
        }

        // Decay pass
        let decay_params = self.decay_params.with_dt(dt);
        self.decay
            .dispatch(&self.device, &mut encoder, &decay_params, &self.accum);

        // Spectral resolve pass: accumulation textures → HDR texture
        self.spectral_resolve.render(
            &self.device,
            &mut encoder,
            &self.hdr,
            &self.spectral_resolve_params,
            &self.accum,
        );

        // Composite pass: HDR texture → sRGB display
        self.composite.render(
            &self.device,
            &mut encoder,
            &view,
            &self.composite_params,
            &self.hdr,
        );

        // egui overlay pass
        if let Some(egui) = egui {
            for (id, delta) in &egui.textures_delta.set {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }

            self.egui_renderer.update_buffers(
                &self.device,
                &self.queue,
                &mut encoder,
                &egui.primitives,
                &egui.screen_descriptor,
            );

            render_egui_pass(&self.egui_renderer, &mut encoder, &view, egui);

            for id in &egui.textures_delta.free {
                self.egui_renderer.free_texture(id);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}

/// Render egui overlay in a separate function to avoid lifetime conflicts
/// between the encoder borrow (for the render pass) and the renderer borrow
/// (through `self`) in wgpu 27 where `RenderPass` borrows the encoder.
fn render_egui_pass(
    renderer: &egui_wgpu::Renderer,
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    egui: &EguiRenderOutput,
) {
    let mut rpass = encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        })
        .forget_lifetime();

    renderer.render(&mut rpass, &egui.primitives, &egui.screen_descriptor);
}

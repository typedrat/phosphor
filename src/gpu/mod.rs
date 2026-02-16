pub mod accumulation;
pub mod beam_write;
pub mod composite;
pub mod decay;
pub mod faceplate_scatter;
pub mod profiler;
pub mod spectral_resolve;

use std::sync::Arc;
use std::time::Instant;

use phosphor_data::spectral::SPECTRAL_BANDS;

/// Time constant cutoff for classifying decay terms into tiers.
/// Exponentials with tau < TAU_CUTOFF are "instantaneous" (tier 1),
/// those >= TAU_CUTOFF are "slow" (tier 2).
pub const TAU_CUTOFF: f32 = 1e-4; // 100 µs

use winit::window::Window;

use crate::beam::BeamSample;
use crate::phosphor::PhosphorType;
use crate::types::Resolution;
use crate::ui::EguiRenderOutput;

const SPECTRAL_CONSTANTS: &[(&str, f64)] = &[("SPECTRAL_BANDS", SPECTRAL_BANDS as f64)];

use self::accumulation::{AccumulationBuffer, HdrBuffer};
use self::beam_write::{BeamParams, BeamWritePipeline, EmissionParams};
use self::composite::{CompositeParams, CompositePipeline, TonemapMode};
use self::decay::{DecayParams, DecayPipeline};
use self::faceplate_scatter::{
    FaceplateScatterParams, FaceplateScatterPipeline, FaceplateScatterTextures,
};
use self::profiler::{GpuProfiler, GpuQuery};
use self::spectral_resolve::{SpectralResolveParams, SpectralResolvePipeline};

pub struct GpuState {
    // Drop order matters: GPU resources that reference the surface/device must
    // drop before the surface, which must drop before the device/instance.
    pub egui_renderer: egui_wgpu::Renderer,
    pub profiler: Option<GpuProfiler>,
    pub composite: CompositePipeline,
    pub composite_params: CompositeParams,
    pub faceplate_scatter: FaceplateScatterPipeline,
    pub faceplate_scatter_textures: FaceplateScatterTextures,
    pub faceplate_scatter_params: FaceplateScatterParams,
    pub spectral_resolve: SpectralResolvePipeline,
    pub spectral_resolve_params: SpectralResolveParams,
    pub decay: DecayPipeline,
    pub decay_params: DecayParams,
    pub beam_write: BeamWritePipeline,
    pub beam_params: BeamParams,
    pub emission_params: EmissionParams,
    pub hdr: HdrBuffer,
    pub accum: AccumulationBuffer,
    pub surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
    pub queue: wgpu::Queue,
    pub device: wgpu::Device,
    pub adapter: wgpu::Adapter,
    pub instance: wgpu::Instance,
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

        let mut features = wgpu::Features::FLOAT32_FILTERABLE;
        let has_timestamps = GpuProfiler::supports_timestamps(&adapter);
        if has_timestamps {
            features |= wgpu::Features::TIMESTAMP_QUERY;
            features |= wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        }

        let adapter_limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("phosphor"),
            required_features: features,
            required_limits: wgpu::Limits {
                max_storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size,
                max_buffer_size: adapter_limits.max_buffer_size,
                ..wgpu::Limits::default()
            },
            ..Default::default()
        }))
        .expect("failed to create GPU device");

        let profiler = if has_timestamps {
            Some(GpuProfiler::new(&device, &queue))
        } else {
            None
        };

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

        let buffer_res = Resolution::new(surface_config.width, surface_config.height);

        // Default: P1 has 2 slow exponentials, no power law, no instant → 32 layers.
        // Recalculated on phosphor switch via switch_phosphor().
        let default_layers = accumulation::accum_layer_count(2, false, false);
        let accum = AccumulationBuffer::new(&device, buffer_res, default_layers);

        let beam_write = BeamWritePipeline::new(&device);

        // Default beam parameters — will be configurable via UI later
        let beam_params = BeamParams::new(
            1.5,  // sigma_core (pixels)
            6.0,  // sigma_halo (pixels)
            0.03, // halo_fraction
            buffer_res.width,
            buffer_res.height,
        );

        let decay = DecayPipeline::new(&device);

        // Default P1 green phosphor decay (Selomulya bi-exponential).
        // Will be recalculated on phosphor switch via switch_phosphor().
        let default_terms = &[
            phosphor_data::DecayTerm::Exponential {
                amplitude: 6.72,
                tau: 0.00288,
            },
            phosphor_data::DecayTerm::Exponential {
                amplitude: 1.0,
                tau: 0.0151,
            },
        ];
        let decay_params = DecayParams::from_terms(default_terms, TAU_CUTOFF);

        // Default P1 green phosphor emission
        let emission_params =
            EmissionParams::from_phosphor(&[1.0 / 16.0; 16], default_terms, TAU_CUTOFF);

        let hdr = HdrBuffer::new(&device, buffer_res);

        let spectral_resolve = SpectralResolvePipeline::new(&device);
        let spectral_resolve_params = SpectralResolveParams::new();

        let faceplate_scatter = FaceplateScatterPipeline::new(&device);
        let faceplate_scatter_textures = FaceplateScatterTextures::new(&device, buffer_res);
        let faceplate_scatter_params = FaceplateScatterParams::default();

        let composite = CompositePipeline::new(&device, format);
        let tonemap_mode = if hdr_output {
            TonemapMode::None
        } else {
            TonemapMode::default()
        };
        let composite_params = CompositeParams::new(1.0, tonemap_mode);

        let egui_renderer = egui_wgpu::Renderer::new(&device, format, Default::default());

        Self {
            instance,
            adapter,
            device,
            queue,
            profiler,
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
            faceplate_scatter,
            faceplate_scatter_textures,
            faceplate_scatter_params,
            composite,
            composite_params,
            egui_renderer,
            hdr_output,
            last_frame: Instant::now(),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32, buffer_scale: f32) {
        if width > 0 && height > 0 {
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
            let bw = ((width as f32) * buffer_scale).round() as u32;
            let bh = ((height as f32) * buffer_scale).round() as u32;
            self.resize_buffers(Resolution::new(bw.max(1), bh.max(1)));
        }
    }

    /// Resize the internal accumulation, HDR, and scatter buffers without
    /// touching the swapchain surface. Used when the buffer scale changes.
    pub fn resize_buffers(&mut self, resolution: Resolution) {
        self.accum.resize(&self.device, resolution);
        self.hdr.resize(&self.device, resolution);
        self.faceplate_scatter_textures
            .resize(&self.device, resolution);
        self.beam_params.width = resolution.width;
        self.beam_params.height = resolution.height;
    }

    /// Reconfigure GPU state for a new phosphor type. Rebuilds decay params,
    /// emission params, spectral resolve params, and reallocates the
    /// accumulation buffer if the layer count changed.
    pub fn switch_phosphor(&mut self, phosphor: &PhosphorType) {
        let terms = &phosphor.fluorescence.decay_terms;
        let class = phosphor_data::classify_decay_terms(terms, TAU_CUTOFF);
        let layers = accumulation::accum_layer_count(
            class.slow_exp_count,
            class.has_power_law,
            class.instant_exp_count > 0,
        );

        if layers != self.accum.layers {
            self.accum =
                AccumulationBuffer::new(&self.device, self.accum.resolution, layers.max(1));
        } else {
            // Zero the buffer even if same size — old phosphor's data is invalid
            self.queue.write_buffer(
                &self.accum.buffer,
                0,
                &vec![0u8; self.accum.buffer.size() as usize],
            );
        }

        self.decay_params = DecayParams::from_terms(terms, TAU_CUTOFF);
        self.emission_params = EmissionParams::from_phosphor(
            &phosphor.fluorescence.emission_weights,
            terms,
            TAU_CUTOFF,
        );
        self.spectral_resolve_params
            .update_from_phosphor(terms, TAU_CUTOFF);
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

        let beam_sample_count = samples.len() as u32;
        if let Some(profiler) = &mut self.profiler {
            profiler.read_back(&self.device, beam_sample_count);
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });

        // Start of GPU work
        if let Some(profiler) = &self.profiler {
            profiler.timestamp(&mut encoder, GpuQuery::FrameStart);
        }

        // Beam write pass
        if !samples.is_empty() {
            let params = self.beam_params.with_sample_count(samples.len() as u32);
            self.beam_write.dispatch(
                &self.device,
                &mut encoder,
                samples,
                &params,
                &self.emission_params,
                &self.accum,
            );
        }
        if let Some(profiler) = &self.profiler {
            profiler.timestamp(&mut encoder, GpuQuery::AfterBeamWrite);
        }

        // Spectral resolve pass: accumulation textures → HDR texture.
        // Runs before decay so that newly deposited energy (including tier-1
        // instant emission) is displayed at full brightness this frame.
        self.spectral_resolve.render(
            &self.device,
            &mut encoder,
            &self.hdr,
            &self.spectral_resolve_params,
            &self.accum,
        );
        if let Some(profiler) = &self.profiler {
            profiler.timestamp(&mut encoder, GpuQuery::AfterSpectralResolve);
        }

        // Decay pass: runs after spectral resolve so that tier-1 instant
        // layers are read before being cleared for the next frame.
        let decay_params = self.decay_params.with_dt(dt);
        self.decay
            .dispatch(&self.device, &mut encoder, &decay_params, &self.accum);
        if let Some(profiler) = &self.profiler {
            profiler.timestamp(&mut encoder, GpuQuery::AfterDecay);
        }

        // FaceplateScatter passes: downsample HDR → blur H → blur V
        self.faceplate_scatter.render(
            &self.device,
            &mut encoder,
            &self.hdr,
            &self.faceplate_scatter_textures,
            &self.faceplate_scatter_params,
        );
        if let Some(profiler) = &self.profiler {
            profiler.timestamp(&mut encoder, GpuQuery::AfterFaceplateScatter);
        }

        // Composite pass: HDR + faceplate_scatter → display
        self.composite.render(
            &self.device,
            &mut encoder,
            &view,
            &self.composite_params,
            &self.hdr,
            &self.faceplate_scatter_textures,
        );
        if let Some(profiler) = &self.profiler {
            profiler.timestamp(&mut encoder, GpuQuery::AfterComposite);
            // Resolve all queries into the buffer for reading next frame
            profiler.resolve(&mut encoder);
        }

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
pub fn render_egui_pass(
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

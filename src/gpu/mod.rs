pub mod accumulation;
pub mod beam_write;
pub mod decay;
pub mod tonemap;

use std::sync::Arc;
use std::time::Instant;

use winit::window::Window;

use crate::beam::BeamSample;

use self::accumulation::AccumulationBuffer;
use self::beam_write::{BeamParams, BeamWritePipeline, EmissionParams};
use self::decay::{DecayParams, DecayPipeline};
use self::tonemap::{TonemapMode, TonemapParams, TonemapPipeline};

pub struct GpuState {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
    pub accum: AccumulationBuffer,
    pub beam_write: BeamWritePipeline,
    pub beam_params: BeamParams,
    pub emission_params: EmissionParams,
    pub decay: DecayPipeline,
    pub decay_params: DecayParams,
    pub tonemap: TonemapPipeline,
    pub tonemap_params: TonemapParams,
    last_frame: Instant,
}

impl GpuState {
    pub fn new(window: Arc<dyn Window>) -> Self {
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

        let size = window.surface_size();
        let surface_caps = surface.get_capabilities(&adapter);
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

        let tonemap = TonemapPipeline::new(&device, format);
        let tonemap_params = TonemapParams::new(1.0, TonemapMode::default());

        Self {
            device,
            queue,
            surface,
            surface_config,
            accum,
            beam_write,
            beam_params,
            emission_params,
            decay,
            decay_params,
            tonemap,
            tonemap_params,
            last_frame: Instant::now(),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
            self.accum.resize(&self.device, width, height);
            self.beam_params.width = width;
            self.beam_params.height = height;
        }
    }

    pub fn render(&mut self, samples: &[BeamSample]) -> Result<(), wgpu::SurfaceError> {
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

        // Tonemap pass: spectral accumulation → sRGB display
        self.tonemap.render(
            &self.device,
            &mut encoder,
            &view,
            &self.tonemap_params,
            &self.accum,
        );

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}

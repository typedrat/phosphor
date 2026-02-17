use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::accumulation::HdrBuffer;
use super::faceplate_scatter::FaceplateScatterTextures;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum TonemapMode {
    #[default]
    Reinhard = 0,
    Aces = 1,
    Clamp = 2,
    /// HDR passthrough — applies exposure only, no tonemapping compression.
    /// Use when the swapchain surface is an HDR format (e.g. Rgba16Float).
    None = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CompositeParams {
    pub exposure: f32,
    tonemap_mode: u32,
    pub faceplate_scatter_intensity: f32,
    pub curvature: f32,
    pub glass_tint: [f32; 3],
    pub edge_falloff: f32,
    pub viewport_size: [f32; 2],
    pub viewport_offset: [f32; 2],
}

impl CompositeParams {
    pub fn new(exposure: f32, mode: TonemapMode) -> Self {
        Self {
            exposure,
            tonemap_mode: mode as u32,
            faceplate_scatter_intensity: 0.15,
            curvature: 0.0,
            glass_tint: [0.92, 0.95, 0.92],
            edge_falloff: 0.0,
            viewport_size: [1.0, 1.0],
            viewport_offset: [0.0; 2],
        }
    }

    pub fn set_mode(&mut self, mode: TonemapMode) {
        self.tonemap_mode = mode as u32;
    }

    pub fn mode(&self) -> TonemapMode {
        match self.tonemap_mode {
            1 => TonemapMode::Aces,
            2 => TonemapMode::Clamp,
            3 => TonemapMode::None,
            _ => TonemapMode::Reinhard,
        }
    }
}

pub struct CompositePipeline {
    pipeline: wgpu::RenderPipeline,
    params_bind_group_layout: wgpu::BindGroupLayout,
    hdr_bind_group_layout: wgpu::BindGroupLayout,
    faceplate_scatter_bind_group_layout: wgpu::BindGroupLayout,
    linear_sampler: wgpu::Sampler,
}

impl CompositePipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });

        let params_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("composite_params"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let texture_and_sampler_entries = |label| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            })
        };

        let hdr_bind_group_layout = texture_and_sampler_entries("composite_hdr");
        let faceplate_scatter_bind_group_layout =
            texture_and_sampler_entries("composite_faceplate_scatter");

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("composite_linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite"),
            bind_group_layouts: &[
                &params_bind_group_layout,
                &hdr_bind_group_layout,
                &faceplate_scatter_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            params_bind_group_layout,
            hdr_bind_group_layout,
            faceplate_scatter_bind_group_layout,
            linear_sampler,
        }
    }

    pub fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        params: &CompositeParams,
        hdr: &HdrBuffer,
        faceplate_scatter: &FaceplateScatterTextures,
    ) {
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("composite_params"),
            contents: bytemuck::bytes_of(params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite_params"),
            layout: &self.params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        let hdr_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite_hdr"),
            layout: &self.hdr_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hdr.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        // Faceplate scatter result is in view_a after the blur passes
        let faceplate_scatter_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite_faceplate_scatter"),
            layout: &self.faceplate_scatter_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&faceplate_scatter.view_a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &params_bind_group, &[]);
        pass.set_bind_group(1, &hdr_bind_group, &[]);
        pass.set_bind_group(2, &faceplate_scatter_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

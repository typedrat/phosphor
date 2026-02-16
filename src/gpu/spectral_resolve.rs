use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::phosphor::spectral::CIE_INTEGRATION_WEIGHTS;

use super::SPECTRAL_CONSTANTS;
use super::accumulation::{AccumulationBuffer, HdrBuffer};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct SpectralResolveParams {
    /// CIE x_bar weights packed as 4 vec4s (4 bands per vec4).
    cie_x: [[f32; 4]; 4],
    /// CIE y_bar weights.
    cie_y: [[f32; 4]; 4],
    /// CIE z_bar weights.
    cie_z: [[f32; 4]; 4],
    pub slow_exp_count: u32,
    pub has_power_law: u32,
    pub power_law_alpha: f32,
    pub power_law_beta: f32,
    pub has_instant: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

impl SpectralResolveParams {
    pub fn new() -> Self {
        let mut cie_x = [[0.0f32; 4]; 4];
        let mut cie_y = [[0.0f32; 4]; 4];
        let mut cie_z = [[0.0f32; 4]; 4];

        for (i, &(x, y, z)) in CIE_INTEGRATION_WEIGHTS.iter().enumerate() {
            cie_x[i / 4][i % 4] = x;
            cie_y[i / 4][i % 4] = y;
            cie_z[i / 4][i % 4] = z;
        }

        Self {
            cie_x,
            cie_y,
            cie_z,
            slow_exp_count: 2, // default for P1
            has_power_law: 0,
            power_law_alpha: 0.0,
            power_law_beta: 0.0,
            has_instant: 0,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        }
    }

    pub fn update_from_phosphor(&mut self, terms: &[phosphor_data::DecayTerm], tau_cutoff: f32) {
        let class = phosphor_data::classify_decay_terms(terms, tau_cutoff);
        self.slow_exp_count = class.slow_exp_count as u32;
        self.has_power_law = if class.has_power_law { 1 } else { 0 };
        self.has_instant = if class.instant_exp_count > 0 { 1 } else { 0 };

        // Extract power-law params if present
        for term in terms {
            if let phosphor_data::DecayTerm::PowerLaw { alpha, beta, .. } = term {
                self.power_law_alpha = *alpha;
                self.power_law_beta = *beta;
                break;
            }
        }
    }
}

pub struct SpectralResolvePipeline {
    pipeline: wgpu::RenderPipeline,
    params_bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_group_layout: wgpu::BindGroupLayout,
}

impl SpectralResolvePipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("spectral_resolve"),
            source: wgpu::ShaderSource::Wgsl(include_str!("spectral_resolve.wgsl").into()),
        });

        let params_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("spectral_resolve_params"),
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

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("spectral_resolve_accum"),
                entries: &[
                    // Read-only storage buffer of u32 (bitcast to f32)
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // AccumDims uniform
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("spectral_resolve"),
            bind_group_layouts: &[&params_bind_group_layout, &texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("spectral_resolve"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: SPECTRAL_CONSTANTS,
                    ..Default::default()
                },
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
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: SPECTRAL_CONSTANTS,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba32Float,
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
            texture_bind_group_layout,
        }
    }

    pub fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        hdr: &HdrBuffer,
        params: &SpectralResolveParams,
        accum: &AccumulationBuffer,
    ) {
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spectral_resolve_params"),
            contents: bytemuck::bytes_of(params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spectral_resolve_params"),
            layout: &self.params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        let dims_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spectral_resolve_accum_dims"),
            contents: bytemuck::bytes_of(&accum.dims()),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spectral_resolve_accum"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: accum.buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: dims_buffer.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("spectral_resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &hdr.view,
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
        pass.set_bind_group(1, &texture_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

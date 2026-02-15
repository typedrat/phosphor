use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::phosphor::spectral::CIE_INTEGRATION_WEIGHTS;

use super::SPECTRAL_CONSTANTS;
use super::accumulation::AccumulationBuffer;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum TonemapMode {
    #[default]
    Reinhard = 0,
    Aces = 1,
    Clamp = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct TonemapParams {
    /// CIE x_bar weights packed as 4 vec4s (4 bands per vec4, matching texture RGBA).
    cie_x: [[f32; 4]; 4],
    /// CIE y_bar weights.
    cie_y: [[f32; 4]; 4],
    /// CIE z_bar weights.
    cie_z: [[f32; 4]; 4],
    pub exposure: f32,
    tonemap_mode: u32,
    _pad1: f32,
    _pad2: f32,
}

impl TonemapParams {
    pub fn new(exposure: f32, mode: TonemapMode) -> Self {
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
            exposure,
            tonemap_mode: mode as u32,
            _pad1: 0.0,
            _pad2: 0.0,
        }
    }

    pub fn set_mode(&mut self, mode: TonemapMode) {
        self.tonemap_mode = mode as u32;
    }

    pub fn mode(&self) -> TonemapMode {
        match self.tonemap_mode {
            1 => TonemapMode::Aces,
            2 => TonemapMode::Clamp,
            _ => TonemapMode::Reinhard,
        }
    }
}

pub struct TonemapPipeline {
    pipeline: wgpu::RenderPipeline,
    params_bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_group_layout: wgpu::BindGroupLayout,
}

impl TonemapPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tonemap"),
            source: wgpu::ShaderSource::Wgsl(include_str!("tonemap.wgsl").into()),
        });

        let params_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("tonemap_params"),
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
                label: Some("tonemap_textures"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                }],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tonemap"),
            bind_group_layouts: &[&params_bind_group_layout, &texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tonemap"),
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
            texture_bind_group_layout,
        }
    }

    pub fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        params: &TonemapParams,
        accum: &AccumulationBuffer,
    ) {
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tonemap_params"),
            contents: bytemuck::bytes_of(params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tonemap_params"),
            layout: &self.params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tonemap_textures"),
            layout: &self.texture_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&accum.view),
            }],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tonemap"),
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
        pass.set_bind_group(1, &texture_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

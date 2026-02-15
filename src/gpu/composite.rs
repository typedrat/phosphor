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
    _pad: f32,
}

impl CompositeParams {
    pub fn new(exposure: f32, mode: TonemapMode, faceplate_scatter_intensity: f32) -> Self {
        Self {
            exposure,
            tonemap_mode: mode as u32,
            faceplate_scatter_intensity,
            _pad: 0.0,
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

        let hdr_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("composite_hdr_texture"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });

        let faceplate_scatter_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("composite_faceplate_scatter_texture"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
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
            label: Some("composite_hdr_texture"),
            layout: &self.hdr_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&hdr.view),
            }],
        });

        // FaceplateScatter result is in view_a after the blur passes
        let faceplate_scatter_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite_faceplate_scatter_texture"),
            layout: &self.faceplate_scatter_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&faceplate_scatter.view_a),
            }],
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

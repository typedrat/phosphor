use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::accumulation::HdrBuffer;

/// Half-resolution texture pair for ping-pong faceplate_scatter blur.
pub struct FaceplateScatterTextures {
    #[allow(dead_code)] // kept alive for view_a
    pub tex_a: wgpu::Texture,
    pub view_a: wgpu::TextureView,
    #[allow(dead_code)] // kept alive for view_b
    pub tex_b: wgpu::Texture,
    pub view_b: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl FaceplateScatterTextures {
    pub fn new(device: &wgpu::Device, full_width: u32, full_height: u32) -> Self {
        let width = (full_width / 2).max(1);
        let height = (full_height / 2).max(1);

        let create = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };

        let tex_a = create("faceplate_scatter_a");
        let tex_b = create("faceplate_scatter_b");
        let view_a = tex_a.create_view(&wgpu::TextureViewDescriptor::default());
        let view_b = tex_b.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            tex_a,
            view_a,
            tex_b,
            view_b,
            width,
            height,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, full_width: u32, full_height: u32) {
        let w = (full_width / 2).max(1);
        let h = (full_height / 2).max(1);
        if w == self.width && h == self.height {
            return;
        }
        *self = Self::new(device, full_width, full_height);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DownsampleParams {
    pub threshold: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct BlurParams {
    pub direction: [f32; 2],
    pub sigma: f32,
    _pad: f32,
}

/// User-facing faceplate_scatter parameters.
pub struct FaceplateScatterParams {
    /// Luminance threshold below which pixels don't scatter.
    pub threshold: f32,
    /// Blur sigma in texels at half resolution.
    pub sigma: f32,
    /// Intensity of the faceplate_scatter effect added to the image.
    pub intensity: f32,
}

impl Default for FaceplateScatterParams {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            sigma: 4.0,
            intensity: 0.15,
        }
    }
}

pub struct FaceplateScatterPipeline {
    downsample_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    params_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
}

impl FaceplateScatterPipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let faceplate_scatter_format = wgpu::TextureFormat::Rgba16Float;

        // Shared bind group layouts — both passes use the same pattern:
        // group(0) = uniform buffer, group(1) = texture
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("faceplate_scatter_params"),
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

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("faceplate_scatter_texture"),
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
            label: Some("faceplate_scatter"),
            bind_group_layouts: &[&params_layout, &texture_layout],
            push_constant_ranges: &[],
        });

        // Downsample pipeline
        let downsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("faceplate_scatter_downsample"),
            source: wgpu::ShaderSource::Wgsl(include_str!("faceplate_scatter_downsample.wgsl").into()),
        });

        let downsample_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("faceplate_scatter_downsample"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &downsample_shader,
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
                module: &downsample_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: faceplate_scatter_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        // Blur pipeline (same layout, different shader)
        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("faceplate_scatter_blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("faceplate_scatter_blur.wgsl").into()),
        });

        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("faceplate_scatter_blur"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blur_shader,
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
                module: &blur_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: faceplate_scatter_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        Self {
            downsample_pipeline,
            blur_pipeline,
            params_layout,
            texture_layout,
        }
    }

    /// Run all faceplate_scatter passes: downsample HDR → blur H → blur V.
    /// Result ends up in `textures.view_a`.
    pub fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        hdr: &HdrBuffer,
        textures: &FaceplateScatterTextures,
        params: &FaceplateScatterParams,
    ) {
        // Pass 1: Downsample HDR → faceplate_scatter_a
        self.render_downsample(
            device,
            encoder,
            &hdr.view,
            &textures.view_a,
            params.threshold,
        );

        // Pass 2: Blur horizontal faceplate_scatter_a → faceplate_scatter_b
        self.render_blur(
            device,
            encoder,
            &textures.view_a,
            &textures.view_b,
            [1.0, 0.0],
            params.sigma,
        );

        // Pass 3: Blur vertical faceplate_scatter_b → faceplate_scatter_a
        self.render_blur(
            device,
            encoder,
            &textures.view_b,
            &textures.view_a,
            [0.0, 1.0],
            params.sigma,
        );
    }

    fn render_downsample(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        threshold: f32,
    ) {
        let uniforms = DownsampleParams {
            threshold,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("faceplate_scatter_downsample_params"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let params_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("faceplate_scatter_downsample_params"),
            layout: &self.params_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        let texture_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("faceplate_scatter_downsample_texture"),
            layout: &self.texture_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(src),
            }],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("faceplate_scatter_downsample"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
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

        pass.set_pipeline(&self.downsample_pipeline);
        pass.set_bind_group(0, &params_bg, &[]);
        pass.set_bind_group(1, &texture_bg, &[]);
        pass.draw(0..3, 0..1);
    }

    fn render_blur(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        direction: [f32; 2],
        sigma: f32,
    ) {
        let uniforms = BlurParams {
            direction,
            sigma,
            _pad: 0.0,
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("faceplate_scatter_blur_params"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let params_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("faceplate_scatter_blur_params"),
            layout: &self.params_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        let texture_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("faceplate_scatter_blur_texture"),
            layout: &self.texture_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(src),
            }],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("faceplate_scatter_blur"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
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

        pass.set_pipeline(&self.blur_pipeline);
        pass.set_bind_group(0, &params_bg, &[]);
        pass.set_bind_group(1, &texture_bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

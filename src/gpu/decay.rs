use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::SPECTRAL_CONSTANTS;
use super::accumulation::AccumulationBuffer;

pub const MAX_DECAY_TERMS: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DecayTermGpu {
    pub amplitude: f32,
    pub param1: f32,    // tau (exp) or alpha (power_law)
    pub param2: f32,    // 0.0 (exp) or beta (power_law)
    pub type_flag: f32, // 0.0 = exponential, 1.0 = power_law
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DecayParams {
    pub dt: f32,
    pub threshold: f32,
    pub tau_cutoff: f32,
    pub term_count: u32,
    pub terms: [DecayTermGpu; MAX_DECAY_TERMS],
    pub slow_exp_count: u32,
    pub has_power_law: u32,
    pub has_instant: u32,
    pub _pad: u32,
}

impl DecayParams {
    pub fn from_terms(terms: &[phosphor_data::DecayTerm], tau_cutoff: f32) -> Self {
        let mut gpu_terms = [DecayTermGpu::zeroed(); MAX_DECAY_TERMS];
        let class = phosphor_data::classify_decay_terms(terms, tau_cutoff);

        // Pack slow exponentials first (tier 2), then power-law (tier 3)
        let mut idx = 0;
        for term in terms {
            if let phosphor_data::DecayTerm::Exponential { amplitude, tau } = term
                && *tau >= tau_cutoff
            {
                gpu_terms[idx] = DecayTermGpu {
                    amplitude: *amplitude,
                    param1: *tau,
                    param2: 0.0,
                    type_flag: 0.0,
                };
                idx += 1;
            }
        }
        for term in terms {
            if let phosphor_data::DecayTerm::PowerLaw {
                amplitude,
                alpha,
                beta,
            } = term
            {
                gpu_terms[idx] = DecayTermGpu {
                    amplitude: *amplitude,
                    param1: *alpha,
                    param2: *beta,
                    type_flag: 1.0,
                };
                idx += 1;
            }
        }

        Self {
            dt: 0.0,
            threshold: 1e-6,
            tau_cutoff,
            term_count: idx as u32,
            terms: gpu_terms,
            slow_exp_count: class.slow_exp_count as u32,
            has_power_law: if class.has_power_law { 1 } else { 0 },
            has_instant: if class.instant_exp_count > 0 { 1 } else { 0 },
            _pad: 0,
        }
    }

    pub fn with_dt(mut self, dt: f32) -> Self {
        self.dt = dt;
        self
    }
}

pub struct DecayPipeline {
    pipeline: wgpu::ComputePipeline,
    params_bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_group_layout: wgpu::BindGroupLayout,
}

impl DecayPipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("decay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("decay.wgsl").into()),
        });

        let params_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("decay_params"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
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
                label: Some("decay_accum"),
                entries: &[
                    // Flat storage buffer of u32 (bitcast to/from f32)
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // AccumDims uniform
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
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
            label: Some("decay"),
            bind_group_layouts: &[&params_bind_group_layout, &texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("decay"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: SPECTRAL_CONSTANTS,
                ..Default::default()
            },
            cache: None,
        });

        Self {
            pipeline,
            params_bind_group_layout,
            texture_bind_group_layout,
        }
    }

    pub fn dispatch(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        params: &DecayParams,
        accum: &AccumulationBuffer,
    ) {
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("decay_params"),
            contents: bytemuck::bytes_of(params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decay_params"),
            layout: &self.params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        let dims_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("decay_accum_dims"),
            contents: bytemuck::bytes_of(&accum.dims()),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decay_accum"),
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

        let workgroups_x = accum.resolution.width.div_ceil(16);
        let workgroups_y = accum.resolution.height.div_ceil(16);

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("decay"),
            ..Default::default()
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &params_bind_group, &[]);
        pass.set_bind_group(1, &texture_bind_group, &[]);
        pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
    }
}

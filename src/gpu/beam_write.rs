use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::SPECTRAL_CONSTANTS;
use super::accumulation::AccumulationBuffer;
use crate::beam::BeamSample;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct BeamParams {
    pub sigma_core: f32,
    pub sigma_halo: f32,
    pub halo_fraction: f32,
    pub sample_count: u32,
    pub width: u32,
    pub height: u32,
    _pad0: u32,
    _pad1: u32,
}

impl BeamParams {
    pub fn new(
        sigma_core: f32,
        sigma_halo: f32,
        halo_fraction: f32,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            sigma_core,
            sigma_halo,
            halo_fraction,
            sample_count: 0,
            width,
            height,
            _pad0: 0,
            _pad1: 0,
        }
    }

    pub fn with_sample_count(mut self, count: u32) -> Self {
        self.sample_count = count;
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct EmissionParams {
    pub slow_exp_count: u32,
    pub has_power_law: u32,
    /// Sum of A*tau for tier-1 (instantaneous) exponentials — total integrated
    /// energy of the fast decay channels, deposited as a one-frame scalar.
    pub instant_energy_total: f32,
    pub has_instant: u32,
}

impl EmissionParams {
    pub fn from_phosphor(terms: &[phosphor_data::DecayTerm], tau_cutoff: f32) -> Self {
        let class = phosphor_data::classify_decay_terms(terms, tau_cutoff);

        // Total integrated energy of tier-1 terms: ∫₀^∞ A·exp(-t/τ) dt = A·τ
        let instant_total: f32 = terms
            .iter()
            .filter_map(|t| match t {
                phosphor_data::DecayTerm::Exponential { amplitude, tau } if *tau < tau_cutoff => {
                    Some(amplitude * tau)
                }
                _ => None,
            })
            .sum();

        Self {
            slow_exp_count: class.slow_exp_count as u32,
            has_power_law: if class.has_power_law { 1 } else { 0 },
            instant_energy_total: instant_total,
            has_instant: if class.instant_exp_count > 0 { 1 } else { 0 },
        }
    }
}

pub struct BeamWritePipeline {
    pipeline: wgpu::ComputePipeline,
    params_bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_group_layout: wgpu::BindGroupLayout,
}

impl BeamWritePipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("beam_write"),
            source: wgpu::ShaderSource::Wgsl(include_str!("beam_write.wgsl").into()),
        });

        let params_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("beam_write_params"),
                entries: &[
                    // samples storage buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // params uniform
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
                    // emission uniform
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
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

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("beam_write_accum"),
                entries: &[
                    // Flat storage buffer of atomic<u32>
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
            label: Some("beam_write"),
            bind_group_layouts: &[&params_bind_group_layout, &texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("beam_write"),
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
        samples: &[BeamSample],
        params: &BeamParams,
        emission: &EmissionParams,
        accum: &AccumulationBuffer,
    ) {
        if samples.is_empty() {
            return;
        }

        let sample_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("beam_samples"),
            contents: bytemuck::cast_slice(samples),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("beam_params"),
            contents: bytemuck::bytes_of(params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let emission_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("emission_params"),
            contents: bytemuck::bytes_of(emission),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("beam_write_params"),
            layout: &self.params_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: sample_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: emission_buffer.as_entire_binding(),
                },
            ],
        });

        let dims_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("beam_write_accum_dims"),
            contents: bytemuck::bytes_of(&accum.dims()),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("beam_write_accum"),
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

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("beam_write"),
            ..Default::default()
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &params_bind_group, &[]);
        pass.set_bind_group(1, &texture_bind_group, &[]);
        // One workgroup per sample
        pass.dispatch_workgroups(samples.len() as u32, 1, 1);
    }
}

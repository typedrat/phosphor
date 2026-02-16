use bytemuck::{Pod, Zeroable};

use crate::types::Resolution;

/// Compute total accumulation buffer layers given a decay classification.
///
/// Layout (in order):
///   Tier 2: slow_exp_count layers (1 scalar energy per term)
///   Tier 3: 2 layers if has_power_law (1 scalar peak energy + 1 elapsed time)
///   Tier 1: 1 layer if has_instant (1 scalar instant energy)
pub fn accum_layer_count(slow_exp_count: usize, has_power_law: bool, has_instant: bool) -> u32 {
    let mut layers = slow_exp_count;
    if has_power_law {
        layers += 2;
    }
    if has_instant {
        layers += 1;
    }
    layers as u32
}

/// Dimensions uniform passed to shaders that access the flat accumulation buffer.
/// Indexing: `layer * (width * height) + y * width + x`
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct AccumDims {
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub _pad: u32,
}

pub struct AccumulationBuffer {
    pub buffer: wgpu::Buffer,
    pub resolution: Resolution,
    pub layers: u32,
}

impl AccumulationBuffer {
    pub fn new(device: &wgpu::Device, resolution: Resolution, layers: u32) -> Self {
        let Resolution { width, height } = resolution;
        let size = (width as u64) * (height as u64) * (layers as u64) * 4;

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("accumulation"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        log::info!(
            "Accumulation buffer: {layers} layers, {resolution}, {:.1} MB VRAM",
            size as f64 / (1024.0 * 1024.0)
        );

        Self {
            buffer,
            resolution,
            layers,
        }
    }

    pub fn dims(&self) -> AccumDims {
        AccumDims {
            width: self.resolution.width,
            height: self.resolution.height,
            layers: self.layers,
            _pad: 0,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, resolution: Resolution) {
        if resolution == self.resolution {
            return;
        }
        *self = Self::new(device, resolution, self.layers);
    }
}

/// Intermediate HDR texture between spectral resolve and composite passes.
/// Stores linear sRGB in Rgba32Float, same resolution as the accumulation buffer.
pub struct HdrBuffer {
    // Kept alive for its view.
    #[allow(dead_code)]
    pub texture: wgpu::Texture,

    pub view: wgpu::TextureView,
    pub resolution: Resolution,
}

impl HdrBuffer {
    pub fn new(device: &wgpu::Device, resolution: Resolution) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hdr_buffer"),
            size: wgpu::Extent3d {
                width: resolution.width,
                height: resolution.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            texture,
            view,
            resolution,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, resolution: Resolution) {
        if resolution == self.resolution {
            return;
        }
        *self = Self::new(device, resolution);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p1_layer_count() {
        // P1: 2 slow exponentials (1 scalar each), no power-law, no instant = 2 layers
        assert_eq!(accum_layer_count(2, false, false), 2);
    }

    #[test]
    fn p31_layer_count() {
        // P31: 0 slow exp, 1 power law (1 peak + 1 elapsed), 3 instant (1 scalar) = 3
        assert_eq!(accum_layer_count(0, true, true), 3);
    }

    #[test]
    fn p15_layer_count() {
        // P15: 0 slow exp, no power law, 1 instant (1 scalar) = 1
        assert_eq!(accum_layer_count(0, false, true), 1);
    }
}

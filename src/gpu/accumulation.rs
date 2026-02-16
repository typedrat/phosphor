use bytemuck::{Pod, Zeroable};
use phosphor_data::spectral::SPECTRAL_BANDS;

/// Array layers per decay component (fast or slow): one layer per spectral band.
pub const LAYERS_PER_COMPONENT: u32 = SPECTRAL_BANDS as u32;

/// Total array layers for a single phosphor layer (fast + slow decay).
pub const LAYERS_PER_DECAY_PAIR: u32 = LAYERS_PER_COMPONENT * 2;

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
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}

impl AccumulationBuffer {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, phosphor_layers: u32) -> Self {
        let layers = LAYERS_PER_DECAY_PAIR * phosphor_layers;
        let size = (width as u64) * (height as u64) * (layers as u64) * 4;

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("accumulation"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        log::info!(
            "Accumulation buffer: {layers} layers, {width}x{height}, {:.1} MB VRAM",
            size as f64 / (1024.0 * 1024.0)
        );

        Self {
            buffer,
            width,
            height,
            layers,
        }
    }

    pub fn dims(&self) -> AccumDims {
        AccumDims {
            width: self.width,
            height: self.height,
            layers: self.layers,
            _pad: 0,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        let phosphor_layers = self.layers / LAYERS_PER_DECAY_PAIR;
        *self = Self::new(device, width, height, phosphor_layers);
    }
}

/// Intermediate HDR texture between spectral resolve and composite passes.
/// Stores linear sRGB in Rgba32Float, same resolution as the accumulation buffer.
pub struct HdrBuffer {
    // Kept alive for its view.
    #[allow(dead_code)]
    pub texture: wgpu::Texture,

    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl HdrBuffer {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hdr_buffer"),
            size: wgpu::Extent3d {
                width,
                height,
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
            width,
            height,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        *self = Self::new(device, width, height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_counts_match_bands() {
        assert_eq!(LAYERS_PER_COMPONENT, 16);
        assert_eq!(LAYERS_PER_DECAY_PAIR, 32);
    }
}

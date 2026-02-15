use phosphor_data::spectral::SPECTRAL_BANDS;

/// Array layers per decay component (fast or slow): one layer per spectral band.
pub const LAYERS_PER_COMPONENT: u32 = SPECTRAL_BANDS as u32;

/// Total array layers for a single phosphor layer (fast + slow decay).
pub const LAYERS_PER_DECAY_PAIR: u32 = LAYERS_PER_COMPONENT * 2;

pub struct AccumulationBuffer {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}

impl AccumulationBuffer {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, phosphor_layers: u32) -> Self {
        let layers = LAYERS_PER_DECAY_PAIR * phosphor_layers;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("accumulation"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let vram_bytes = layers as u64 * width as u64 * height as u64 * 4; // 1 × f32
        log::info!(
            "Accumulation buffer: {layers} R32Float layers, {width}x{height}, {:.1} MB VRAM",
            vram_bytes as f64 / (1024.0 * 1024.0)
        );

        Self {
            texture,
            view,
            width,
            height,
            layers,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_counts_match_bands() {
        assert_eq!(LAYERS_PER_COMPONENT, 16);
        assert_eq!(LAYERS_PER_DECAY_PAIR, 32);
    }
}

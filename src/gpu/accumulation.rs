use phosphor_data::spectral::SPECTRAL_BANDS;

/// Number of RGBA32Float textures needed per decay component (fast or slow).
/// Each texture holds 4 spectral bands in its RGBA channels.
pub const TEXTURES_PER_COMPONENT: usize = (SPECTRAL_BANDS + 3) / 4;

/// Textures per phosphor layer (fluorescence or phosphorescence):
/// one set for fast decay + one set for slow decay.
pub const TEXTURES_PER_LAYER: usize = TEXTURES_PER_COMPONENT * 2;

/// Maximum textures for a dual-layer phosphor (fluorescence + phosphorescence).
pub const MAX_TEXTURES: usize = TEXTURES_PER_LAYER * 2;

pub struct AccumulationBuffer {
    pub textures: Vec<wgpu::Texture>,
    pub views: Vec<wgpu::TextureView>,
    pub width: u32,
    pub height: u32,
}

impl AccumulationBuffer {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, layer_count: usize) -> Self {
        let texture_count = TEXTURES_PER_LAYER * layer_count;

        let textures: Vec<wgpu::Texture> = (0..texture_count)
            .map(|i| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(&format!("accumulation_{i}")),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba32Float,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            })
            .collect();

        let views: Vec<wgpu::TextureView> = textures
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();

        let vram_bytes = texture_count as u64 * width as u64 * height as u64 * 16; // 4 × f32
        log::info!(
            "Accumulation buffer: {texture_count} textures, {width}x{height}, {:.1} MB VRAM",
            vram_bytes as f64 / (1024.0 * 1024.0)
        );

        Self {
            textures,
            views,
            width,
            height,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        let layer_count = self.textures.len() / TEXTURES_PER_LAYER;
        *self = Self::new(device, width, height, layer_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_count_matches_bands() {
        assert_eq!(TEXTURES_PER_COMPONENT, 4); // 16 bands / 4 channels
        assert_eq!(TEXTURES_PER_LAYER, 8); // 4 * 2 components
        assert_eq!(MAX_TEXTURES, 16); // 8 * 2 layers
    }
}

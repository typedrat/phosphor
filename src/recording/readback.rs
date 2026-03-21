use wgpu;

/// GPU staging buffer for reading back rendered frames to the CPU.
pub struct ReadbackBuffer {
    pub buffer: wgpu::Buffer,
    pub padded_bytes_per_row: u32,
    pub bytes_per_row: u32,
    pub width: u32,
    pub height: u32,
}

impl ReadbackBuffer {
    /// Create a new readback staging buffer for the given dimensions.
    /// Assumes Rgba16Float format (8 bytes per pixel).
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let bytes_per_row = width * 8;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = bytes_per_row.div_ceil(alignment) * alignment;
        let size = (padded_bytes_per_row * height) as u64;

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("recording_readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            buffer,
            padded_bytes_per_row,
            bytes_per_row,
            width,
            height,
        }
    }

    /// Encode a copy command from the offscreen texture to this staging buffer.
    pub fn copy_from_texture(&self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Map the buffer, call the closure with the raw bytes, then unmap.
    /// Blocks until the GPU copy is complete.
    pub fn read_mapped<F>(&self, device: &wgpu::Device, f: F)
    where
        F: FnOnce(&[u8]),
    {
        let buffer_slice = self.buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        receiver
            .recv()
            .unwrap()
            .expect("Failed to map readback buffer");

        let data = buffer_slice.get_mapped_range();
        f(&data);
        drop(data);
        self.buffer.unmap();
    }
}

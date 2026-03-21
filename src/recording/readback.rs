use wgpu;

/// Double-buffered GPU readback for pipelined frame capture.
///
/// Two staging buffers alternate roles each frame: one receives the GPU copy
/// (write buffer) while the other is mapped and read by the CPU (read buffer).
/// This overlaps GPU rendering with CPU readback, eliminating pipeline stalls.
pub struct DoubleReadbackBuffer {
    buffers: [wgpu::Buffer; 2],
    /// Which buffer index to COPY INTO this frame (the other is being mapped/read).
    write_idx: usize,
    pub padded_bytes_per_row: u32,
    pub bytes_per_row: u32,
    pub width: u32,
    pub height: u32,
    /// Whether we have a pending frame in the read buffer (false for the first frame).
    has_pending: bool,
}

impl DoubleReadbackBuffer {
    /// Create a new double-buffered readback staging buffer for the given dimensions.
    /// Assumes Rgba16Float format (8 bytes per pixel).
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let bytes_per_row = width * 8;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = bytes_per_row.div_ceil(alignment) * alignment;
        let size = (padded_bytes_per_row * height) as u64;

        let buffers = std::array::from_fn(|i| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(if i == 0 {
                    "recording_readback_a"
                } else {
                    "recording_readback_b"
                }),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });

        Self {
            buffers,
            write_idx: 0,
            padded_bytes_per_row,
            bytes_per_row,
            width,
            height,
            has_pending: false,
        }
    }

    /// Encode a copy command from the offscreen texture to the current write buffer.
    pub fn copy_from_texture(&self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffers[self.write_idx],
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

    /// If there is a pending frame in the read buffer, map it, copy the data
    /// into a `Vec<u8>`, unmap, and return it. Returns `None` on the first frame
    /// (before any previous copy has completed).
    pub fn read_pending(&self, device: &wgpu::Device) -> Option<Vec<u8>> {
        if !self.has_pending {
            return None;
        }

        let read_idx = 1 - self.write_idx;
        Some(self.map_and_read(device, read_idx))
    }

    /// Swap write/read buffers and mark that the read buffer now has pending data.
    pub fn advance(&mut self) {
        self.write_idx = 1 - self.write_idx;
        self.has_pending = true;
    }

    /// After the final frame, read the last written buffer (which hasn't been
    /// read yet because the double-buffer is one frame behind).
    pub fn flush(&self, device: &wgpu::Device) -> Option<Vec<u8>> {
        if !self.has_pending {
            return None;
        }

        // After the last advance(), write_idx was swapped, so the buffer that
        // was just written to is now at `1 - self.write_idx`.
        let last_written = 1 - self.write_idx;
        Some(self.map_and_read(device, last_written))
    }

    /// Map, read, and unmap the buffer at the given index.
    fn map_and_read(&self, device: &wgpu::Device, idx: usize) -> Vec<u8> {
        let buffer = &self.buffers[idx];
        let buffer_slice = buffer.slice(..);
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
        let vec = data.to_vec();
        drop(data);
        buffer.unmap();
        vec
    }
}

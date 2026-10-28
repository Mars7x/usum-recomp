//! Per-submission staging snapshots, independent of mutable render targets.
use super::*;
use std::sync::OnceLock;

/// A frozen color/depth snapshot queued after a raster batch. No CPU wait or
/// mapping occurs until `read` is called. The target may be reused immediately;
/// every ticket owns separate staging buffers and caches its result once.
#[derive(Debug)]
pub struct PendingReadback {
    device: wgpu::Device,
    submission: wgpu::SubmissionIndex,
    color: wgpu::Buffer,
    depth: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_row: u32,
    result: OnceLock<Result<ColorDepthReadback, String>>,
}

#[derive(Debug)]
pub struct ColorDepthReadback {
    pub color: Vec<u8>,
    pub depth: Vec<f32>,
}

impl PendingReadback {
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn read(&self) -> Result<&ColorDepthReadback, String> {
        self.result.get_or_init(|| self.materialize()).as_ref().map_err(Clone::clone)
    }

    fn materialize(&self) -> Result<ColorDepthReadback, String> {
        let color_slice = self.color.slice(..);
        let depth_slice = self.depth.slice(..);
        let (color_send, color_receive) = std::sync::mpsc::sync_channel(1);
        let (depth_send, depth_receive) = std::sync::mpsc::sync_channel(1);
        color_slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = color_send.send(r);
        });
        depth_slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = depth_send.send(r);
        });
        // Always unmap both buffers, including on any polling/mapping failure.
        let result = (|| {
            self.device
                .poll(wgpu::PollType::Wait { submission_index: Some(self.submission.clone()), timeout: None })
                .map_err(|e| e.to_string())?;
            color_receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
            depth_receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
            let color_mapped = color_slice.get_mapped_range().map_err(|e| e.to_string())?;
            let depth_mapped = depth_slice.get_mapped_range().map_err(|e| e.to_string())?;
            let row_bytes = self.width as usize * 4;
            let mut color = vec![0; row_bytes * self.height as usize];
            let mut depth = Vec::with_capacity(self.width as usize * self.height as usize);
            for row in 0..self.height as usize {
                let range = row * self.padded_row as usize..row * self.padded_row as usize + row_bytes;
                color[row * row_bytes..][..row_bytes].copy_from_slice(&color_mapped[range.clone()]);
                depth.extend(depth_mapped[range].as_chunks::<4>().0.iter().map(|b| f32::from_ne_bytes(*b)));
            }
            Ok(ColorDepthReadback { color, depth })
        })();
        self.color.unmap();
        self.depth.unmap();
        result
    }
}

impl RasterBatch<'_> {
    /// Queue an immutable result without waiting for execution or CPU readback.
    /// Validation errors are checked before returning so the caller can roll
    /// back the guest command list. Staging allocations belong to this ticket,
    /// rather than to the target which subsequent submissions may overwrite.
    pub fn submit_readback(mut self) -> Result<PendingReadback, String> {
        let mut encoder = self.encoder.take().ok_or("Vulkan raster batch was already submitted")?;
        let errors = self.errors.take().ok_or("Vulkan raster batch has no validation scope")?;
        let (width, height) = self.target.dimensions();
        let row_bytes = width.checked_mul(4).ok_or("render target row is too large")?;
        let padded_row = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let size = u64::from(padded_row) * u64::from(height);
        let staging = |label| {
            self.renderer.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        let color = staging("PICA frozen color readback");
        let depth = staging("PICA frozen depth readback");
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(height) };
        encoder.copy_texture_to_buffer(
            self.target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &color, layout },
            extent,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target.depth,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            wgpu::TexelCopyBufferInfo { buffer: &depth, layout },
            extent,
        );
        let submission = self.renderer.queue.submit([encoder.finish()]);
        check_errors(errors, "deferred raster batch submission")?;
        Ok(PendingReadback {
            device: self.renderer.device.clone(),
            submission,
            color,
            depth,
            width,
            height,
            padded_row,
            result: OnceLock::new(),
        })
    }
}

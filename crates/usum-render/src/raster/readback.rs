//! Per-submission staging snapshots, independent of mutable render targets.
use super::*;
use std::sync::OnceLock;

pub(super) struct ResolvePipeline {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl ResolvePipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native framebuffer readback"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
@group(0) @binding(0) var color: texture_2d<f32>;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var<storage, read_write> colors: array<u32>;
@group(0) @binding(3) var<storage, read_write> depths: array<f32>;
@group(0) @binding(4) var<uniform> params: vec4<u32>;
@compute @workgroup_size(8, 8)
fn resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.x || id.y >= params.y { return; }
    let source = vec2<i32>(id.xy * params.z + vec2<u32>(params.z / 2u));
    let at = id.y * params.w + id.x;
    colors[at] = pack4x8unorm(textureLoad(color, source, 0));
    depths[at] = textureLoad(depth, source, 0);
}
"#
                .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("native framebuffer readback"),
            layout: None,
            module: &shader,
            entry_point: Some("resolve"),
            compilation_options: Default::default(),
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);
        Self { pipeline, layout }
    }
}

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
    /// Freeze native-sized center samples of a scaled framebuffer on the GPU.
    /// The full-resolution target remains resident and can be presented later.
    pub fn submit_resolved_readback(mut self, scale: u32) -> Result<PendingReadback, String> {
        if scale == 0 || !self.target.width.is_multiple_of(scale) || !self.target.height.is_multiple_of(scale) {
            return Err("readback scale must divide both target dimensions".into());
        }
        if scale == 1 {
            return self.submit_readback();
        }
        let mut encoder = self.encoder.take().ok_or("Vulkan raster batch was already submitted")?;
        let errors = self.errors.take().ok_or("Vulkan raster batch has no validation scope")?;
        let (width, height) = (self.target.width / scale, self.target.height / scale);
        let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let size = u64::from(padded_row) * u64::from(height);
        let buffer = |label, usage| {
            self.renderer.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let output_usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        let color_output = buffer("native color resolve", output_usage);
        let depth_output = buffer("native depth resolve", output_usage);
        let stage_usage = wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ;
        let color = buffer("frozen native color", stage_usage);
        let depth = buffer("frozen native depth", stage_usage);
        use wgpu::util::DeviceExt;
        let parameters: Vec<u8> = [width, height, scale, padded_row / 4].into_iter().flat_map(u32::to_ne_bytes).collect();
        let parameters = self.renderer.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native readback parameters"),
            contents: &parameters,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let resolve = &self.renderer.raster_pipelines.resolved_readback;
        let group = self.renderer.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native framebuffer readback"),
            layout: &resolve.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.target.view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.target.depth_view) },
                wgpu::BindGroupEntry { binding: 2, resource: color_output.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: depth_output.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: parameters.as_entire_binding() },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("native framebuffer resolve"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&resolve.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(&color_output, 0, &color, 0, size);
        encoder.copy_buffer_to_buffer(&depth_output, 0, &depth, 0, size);
        let submission = self.renderer.queue.submit([encoder.finish()]);
        check_errors(errors, "resolved raster batch submission")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn resolved_readback_matches_full_color_and_depth_and_survives_target_reuse() {
        let renderer = Renderer::new().unwrap();
        for scale in [1, 2, 3, 6, 8] {
            let (width, height) = (31 * scale, 19 * scale);
            let target = renderer.create_color_target(width, height).unwrap();
            let color: Vec<u8> = (0..width * height * 4).map(|n| (n * 37 + n / 256) as u8).collect();
            let depth: Vec<f32> = (0..width * height).map(|n| (n * 17 % 65536) as f32 / 65535.0).collect();
            renderer.upload_target(&target, &color, &depth).unwrap();
            let mut batch = renderer.begin_batch(&target);
            batch
                .draw(
                    &[
                        RasterVertex::new([-0.91, -0.87, -0.2, 1.0], [1.0, 0.0, 0.5, 1.0]),
                        RasterVertex::new([0.93, -0.89, -0.2, 1.0], [0.0, 1.0, 0.5, 1.0]),
                        RasterVertex::new([0.01, 0.97, -0.2, 1.0], [0.0, 0.0, 1.0, 1.0]),
                    ],
                    Viewport::full(&target),
                    RasterState::default(),
                )
                .unwrap();
            let ticket = batch.submit_resolved_readback(scale).unwrap();
            assert_eq!(ticket.dimensions(), (31, 19));
            let full = renderer.read_color_and_depth_target(&target).unwrap();
            let mut expected_color = Vec::new();
            let mut expected_depth = Vec::new();
            for y in 0..19 {
                for x in 0..31 {
                    let i = ((y * scale + scale / 2) * width + x * scale + scale / 2) as usize;
                    expected_color.extend_from_slice(&full.0[i * 4..i * 4 + 4]);
                    expected_depth.push(full.1[i]);
                }
            }
            renderer.upload_target(&target, &vec![0; color.len()], &vec![1.0; depth.len()]).unwrap();
            drop(target);
            let result = ticket.read().unwrap();
            assert_eq!(result.color, expected_color, "color at scale {scale}");
            assert_eq!(result.depth, expected_depth, "depth at scale {scale}");
            assert!(result.color.iter().any(|&v| v != 0));
            assert!(std::ptr::eq(result, ticket.read().unwrap()));
        }
    }
}

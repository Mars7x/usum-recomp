//! Crop, rotate, downsample and pack display RGB on the GPU.
use super::*;
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Debug)]
pub struct DisplayCopy {
    pub origin: (u32, u32),
    pub size: (u32, u32),
    pub scale: u32,
    pub downscale: (u32, u32),
    pub flip: bool,
}

pub(super) struct DisplayPipeline {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl DisplayPipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("display RGB copy"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
@group(0) @binding(0) var color: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> rgb: array<u32>;
struct Params { output: vec4<u32>, source: vec4<u32> }
@group(0) @binding(2) var<uniform> p: Params;
fn pixel(index: u32) -> u32 {
    let width = p.output.x;
    let height = p.output.y;
    if index >= width * height { return 0u; }
    let x = index % width;
    let y = index / width;
    let source_x = p.source.x * p.output.z + (height - 1u - y) * p.output.w;
    let source_y = p.source.y * p.output.z + select(x, width - 1u - x, p.source.w != 0u) * p.source.z;
    var sum = vec3<u32>(0u);
    for (var dy = 0u; dy < p.source.z; dy += 1u) {
        for (var dx = 0u; dx < p.output.w; dx += 1u) {
            sum += vec3<u32>(round(textureLoad(color, vec2<i32>(vec2<u32>(source_x + dx, source_y + dy)), 0).rgb * 255.0));
        }
    }
    let area = p.output.w * p.source.z;
    let c = (sum + vec3<u32>(area / 2u)) / vec3<u32>(area);
    return c.r | c.g << 8u | c.b << 16u;
}
@compute @workgroup_size(64)
fn copy(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = id.x * 4u;
    if at >= p.output.x * p.output.y { return; }
    let a = pixel(at);
    let b = pixel(at + 1u);
    let c = pixel(at + 2u);
    let d = pixel(at + 3u);
    rgb[id.x * 3u] = a | b << 24u;
    rgb[id.x * 3u + 1u] = b >> 8u | c << 16u;
    rgb[id.x * 3u + 2u] = c >> 16u | d << 8u;
}
"#
                .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("display RGB copy"),
            layout: None,
            module: &shader,
            entry_point: Some("copy"),
            compilation_options: Default::default(),
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);
        Self { pipeline, layout }
    }
}

impl Renderer {
    /// Return tightly packed RGB for a rotated scaled display rectangle. Work
    /// which previously scanned a large, strided CPU image stays on the GPU.
    pub fn read_display_rgb(&self, target: &ColorTarget, copy: DisplayCopy) -> Result<Vec<u8>, String> {
        let DisplayCopy { origin, size, scale, downscale, flip } = copy;
        let (sx, sy) = downscale;
        if scale == 0 || size.0 == 0 || size.1 == 0 || !matches!(sx, 1 | 2) || !matches!(sy, 1 | 2) {
            return Err("invalid display copy dimensions or sampling scale".into());
        }
        let end = |start: u32, size: u32, factor: u32| start.checked_add(size.checked_mul(factor)?)?.checked_mul(scale);
        if !end(origin.0, size.0, sx).is_some_and(|n| n <= target.width)
            || !end(origin.1, size.1, sy).is_some_and(|n| n <= target.height)
        {
            return Err("display copy lies outside the render target".into());
        }
        let width = size.1.checked_mul(scale).ok_or("display width overflow")?;
        let height = size.0.checked_mul(scale).ok_or("display height overflow")?;
        let pixels = u64::from(width) * u64::from(height);
        let groups = pixels.div_ceil(4);
        let buffer_size = groups * 12;
        if buffer_size > self.device.limits().max_storage_buffer_binding_size
            || groups.div_ceil(64) > u64::from(self.device.limits().max_compute_workgroups_per_dimension)
        {
            return Err("display copy exceeds GPU staging limits".into());
        }
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let buffer = |label, usage| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: buffer_size,
                usage,
                mapped_at_creation: false,
            })
        };
        let output = buffer("packed display RGB", wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
        let staging = buffer("display RGB readback", wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST);
        let parameters: Vec<u8> =
            [width, height, scale, sx, origin.0, origin.1, sy, u32::from(flip)].into_iter().flat_map(u32::to_ne_bytes).collect();
        let parameters = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("display copy parameters"),
            contents: &parameters,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let display = &self.raster_pipelines.display_copy;
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("display copy"),
            layout: &display.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&target.view) },
                wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: parameters.as_entire_binding() },
            ],
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("display copy") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("display crop/rotation"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&display.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(groups.div_ceil(64) as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &staging, 0, buffer_size);
        let submission = self.queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = send.send(r);
        });
        let result = (|| {
            self.wait(submission)?;
            receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
            check_errors(errors, "display RGB copy")?;
            let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
            Ok(mapped[..pixels as usize * 3].to_vec())
        })();
        staging.unmap();
        result
    }
}

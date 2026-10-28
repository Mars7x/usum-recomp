//! Upload native framebuffer seeds once and expand color/depth on the GPU.
use super::*;
use wgpu::util::DeviceExt;

pub(super) struct SeedPipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl SeedPipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scaled framebuffer seed"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
@group(0) @binding(0) var color: texture_2d<f32>;
@group(0) @binding(1) var depth: texture_2d<f32>;
@group(0) @binding(2) var<uniform> scale: vec4<u32>;
@vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[index], 0.0, 1.0);
}
struct Pixel { @location(0) color: vec4<f32>, @builtin(frag_depth) depth: f32 }
@fragment fn fragment(@builtin(position) position: vec4<f32>) -> Pixel {
    let source = vec2<i32>(vec2<u32>(position.xy) / vec2<u32>(scale.x));
    return Pixel(textureLoad(color, source, 0), textureLoad(depth, source, 0).r);
}
"#
                .into(),
            ),
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scaled framebuffer seed"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
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
            label: Some("scaled framebuffer seed"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scaled framebuffer seed"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, layout }
    }
}

impl Renderer {
    /// Seed a scaled target from native RGBA8/normalized-depth buffers. Integer
    /// nearest-neighbor replication exactly matches the CPU reference expansion.
    pub fn begin_scaled_batch_from_snapshot<'a>(
        &'a self,
        target: &'a ColorTarget,
        color: &[u8],
        depth: &[f32],
        scale: u32,
    ) -> Result<RasterBatch<'a>, String> {
        if scale == 0 || !target.width.is_multiple_of(scale) || !target.height.is_multiple_of(scale) {
            return Err("snapshot scale must divide both target dimensions".into());
        }
        if scale == 1 {
            return self.begin_batch_from_snapshot(target, color, depth);
        }
        let (width, height) = (target.width / scale, target.height / scale);
        let pixels = width as usize * height as usize;
        if color.len() != pixels * 4 || depth.len() != pixels {
            return Err("native snapshot dimensions do not match the scaled target".into());
        }
        if depth.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) {
            return Err("depth snapshot values must be finite and in 0..=1".into());
        }
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = |label, format| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let native_color = texture("native color seed", wgpu::TextureFormat::Rgba8Unorm);
        let native_depth = texture("native depth seed", wgpu::TextureFormat::R32Float);
        let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) };
        self.queue.write_texture(native_color.as_image_copy(), color, layout, extent);
        let depth_bytes: Vec<u8> = depth.iter().flat_map(|v| v.to_ne_bytes()).collect();
        self.queue.write_texture(native_depth.as_image_copy(), &depth_bytes, layout, extent);
        let color_view = native_color.create_view(&Default::default());
        let depth_view = native_depth.create_view(&Default::default());
        let parameters: Vec<u8> = [scale, 0, 0, 0].into_iter().flat_map(u32::to_ne_bytes).collect();
        let parameters = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("framebuffer seed scale"),
            contents: &parameters,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let seed = &self.raster_pipelines.scaled_seed;
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native framebuffer seed"),
            layout: &seed.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&color_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&depth_view) },
                wgpu::BindGroupEntry { binding: 2, resource: parameters.as_entire_binding() },
            ],
        });
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scaled snapshot raster batch") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("expand native framebuffer seed"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&seed.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        Ok(RasterBatch { renderer: self, target, encoder: Some(encoder), errors: Some(errors), draws: 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn gpu_seed_expansion_preserves_all_color_and_depth_samples() {
        let renderer = Renderer::new().unwrap();
        for scale in [1, 2, 3, 6, 8] {
            let (width, height) = (31, 19);
            let target = renderer.create_color_target(width * scale, height * scale).unwrap();
            let color: Vec<u8> = (0..width * height * 4).map(|n| (n * 37 + n / 256) as u8).collect();
            let depth: Vec<f32> = (0..width * height).map(|n| (n * 17 % 65536) as f32 / 65535.0).collect();
            let batch = renderer.begin_scaled_batch_from_snapshot(&target, &color, &depth, scale).unwrap();
            let actual = batch.finish_and_read_color_and_depth().unwrap();
            for y in 0..height * scale {
                for x in 0..width * scale {
                    let out = (y * width * scale + x) as usize;
                    let input = (y / scale * width + x / scale) as usize;
                    assert_eq!(
                        &actual.0[out * 4..out * 4 + 4],
                        &color[input * 4..input * 4 + 4],
                        "color scale {scale} at {x},{y}"
                    );
                    assert_eq!(actual.1[out], depth[input], "depth scale {scale} at {x},{y}");
                }
            }
        }
    }
}

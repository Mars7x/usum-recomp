//! Immutable render-to-texture color views without a native-resolution round trip.
use super::*;

pub(super) struct ColorFeedback {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl ColorFeedback {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("framebuffer color texture"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
@group(0) @binding(0) var source: texture_2d<f32>;
@vertex fn vertex(@builtin(vertex_index) index:u32)->@builtin(position) vec4<f32> {
    let positions=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
    return vec4<f32>(positions[index],0.0,1.0);
}
@fragment fn fragment(@builtin(position) position:vec4<f32>)->@location(0) vec4<f32> {
    let dimensions=textureDimensions(source);
    return textureLoad(source,vec2<i32>(i32(position.x),i32(dimensions.y-1u-u32(position.y))),0);
}
"#
                .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("framebuffer color texture"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
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
        let layout = pipeline.get_bind_group_layout(0);
        Self { pipeline, layout }
    }
}

impl Renderer {
    pub fn create_color_feedback_texture(
        &self,
        target: &ColorTarget,
        state: SamplerState,
        border: [u8; 4],
    ) -> Result<SampledTexture, String> {
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let extent = wgpu::Extent3d { width: target.width, height: target.height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frozen framebuffer color texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let feedback = &self.raster_pipelines.color_feedback;
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("framebuffer color source"),
            layout: &feedback.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&target.view) }],
        });
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("freeze framebuffer color") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("freeze framebuffer color"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&feedback.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.submit([encoder.finish()]);
        check_errors(errors, "framebuffer color texture")?;
        Ok(SampledTexture { _texture: texture, view, sampler: create_sampler(&self.device, state, Some(border))? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn color_texture_preserves_channels_rows_and_a_reused_source() {
        let renderer = Renderer::new().unwrap();
        let source = renderer.create_color_target(7, 5).unwrap();
        let expected: Vec<u8> = (0..7 * 5 * 4).map(|i| (i * 73 + 19) as u8).collect();
        renderer.upload_target(&source, &expected, &[1.0; 35]).unwrap();
        let texture = renderer.create_color_feedback_texture(&source, SamplerState::default(), [0; 4]).unwrap();
        renderer.upload_target(&source, &[0; 140], &[1.0; 35]).unwrap();
        drop(source);
        let vertex = |x, y, u, v| RasterVertex::new([x, y, 0.0, 1.0], [1.0; 4]).with_texcoord0([u, v]);
        let vertices = [
            vertex(-1.0, -1.0, 0.0, 0.0),
            vertex(1.0, -1.0, 1.0, 0.0),
            vertex(1.0, 1.0, 1.0, 1.0),
            vertex(-1.0, -1.0, 0.0, 0.0),
            vertex(1.0, 1.0, 1.0, 1.0),
            vertex(-1.0, 1.0, 0.0, 1.0),
        ];
        let output = renderer.create_color_target(7, 5).unwrap();
        let mut state = RasterState { depth: DepthState { compare: DepthCompare::Always, write: false }, ..Default::default() };
        state.tev.stages[0] = TevStage::replace(3);
        renderer
            .draw_vertices_with_inputs(
                &output,
                &vertices,
                Viewport::full(&output),
                state,
                FragmentInputs { texture0: Some(&texture), ..Default::default() },
            )
            .unwrap();
        assert_eq!(renderer.read_color_target(&output).unwrap(), expected);
    }
}

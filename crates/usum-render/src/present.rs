//! Present immutable LCD RGB buffers directly on the rendering device.
use crate::{DisplayFrame, Renderer};
use wgpu::util::DeviceExt;

pub struct DisplayScreen<'a> {
    pub frame: &'a DisplayFrame,
    /// Window pixels: left, top, width, height. Sampling matches the CPU
    /// compositor's integer nearest-neighbor mapping, including clipped edges.
    pub rect: [u32; 4],
}

struct Compositor {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
}

fn validate_screens(renderer: &Renderer, screens: &[DisplayScreen<'_>]) -> Result<(), String> {
    for screen in screens {
        let [left, top, width, height] = screen.rect;
        let (sw, sh) = screen.frame.dimensions();
        if !screen.frame.belongs_to(renderer)
            || left.checked_add(width).is_none()
            || top.checked_add(height).is_none()
            || u64::from(width) * u64::from(sw) > u64::from(u32::MAX)
            || u64::from(height) * u64::from(sh) > u64::from(u32::MAX)
        {
            return Err("invalid LCD rectangle or display device".into());
        }
    }
    Ok(())
}

impl Compositor {
    fn new(renderer: &Renderer, format: wgpu::TextureFormat) -> Self {
        let shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resident LCD presentation"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
@group(0) @binding(0) var<storage, read> rgb: array<u32>;
struct Params { source: vec4<u32>, output: vec4<u32> }
@group(0) @binding(1) var<uniform> p: Params;
@vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corners = array<vec2<u32>, 6>(vec2<u32>(0,0), vec2<u32>(1,0), vec2<u32>(0,1),
        vec2<u32>(0,1), vec2<u32>(1,0), vec2<u32>(1,1));
    let pixel = vec2<f32>(p.source.zw + corners[index] * p.output.xy);
    let clip = pixel / vec2<f32>(p.output.zw) * 2.0 - 1.0;
    return vec4<f32>(clip.x, -clip.y, 0.0, 1.0);
}
fn byte(at: u32) -> u32 { return (rgb[at / 4u] >> ((at % 4u) * 8u)) & 255u; }
@fragment fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let local = vec2<u32>(position.xy) - p.source.zw;
    let sample = local * p.source.xy / p.output.xy;
    let at = (sample.y * p.source.x + sample.x) * 3u;
    return vec4<f32>(vec3<f32>(f32(byte(at)), f32(byte(at + 1u)), f32(byte(at + 2u))) / 255.0, 1.0);
}
"#
                .into(),
            ),
        });
        let pipeline = renderer.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("resident LCD presentation"),
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
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);
        Self { pipeline, layout, format }
    }

    fn encode(
        &self,
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        screens: &[DisplayScreen<'_>],
    ) {
        let groups: Vec<_> = screens
            .iter()
            .filter_map(|screen| {
                let [left, top, w, h] = screen.rect;
                if w == 0 || h == 0 || left >= width || top >= height {
                    return None;
                }
                let (sw, sh) = screen.frame.dimensions();
                let parameters: Vec<u8> =
                    [sw, sh, left, top, w, h, width, height].into_iter().flat_map(u32::to_ne_bytes).collect();
                let params = renderer.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("LCD window rectangle"),
                    contents: &parameters,
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let group = renderer.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("resident LCD pixels"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: screen.frame.buffer.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: params.as_entire_binding() },
                    ],
                });
                Some((group, [left, top, w.min(width - left), h.min(height - top)]))
            })
            .collect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("LCD window presentation"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        for (group, [left, top, w, h]) in &groups {
            pass.set_scissor_rect(*left, *top, *w, *h);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    fn capture(&self, renderer: &Renderer, width: u32, height: u32, screens: &[DisplayScreen<'_>]) -> Result<Vec<u8>, String> {
        validate_screens(renderer, screens)?;
        if width == 0
            || height == 0
            || width > renderer.device.limits().max_texture_dimension_2d
            || height > renderer.device.limits().max_texture_dimension_2d
        {
            return Err("invalid LCD capture dimensions".into());
        }
        let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("LCD compositor capture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let staging = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("LCD compositor capture readback"),
            size: u64::from(row) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = renderer.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.encode(renderer, &mut encoder, &texture.create_view(&Default::default()), width, height, screens);
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let submission = renderer.submit([encoder.finish()]);
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let result = (|| {
            renderer
                .device
                .poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None })
                .map_err(|e| e.to_string())?;
            rx.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
            let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
            let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
            for y in 0..height as usize {
                for p in mapped[y * row as usize..y * row as usize + width as usize * 4].as_chunks::<4>().0 {
                    if self.format == wgpu::TextureFormat::Bgra8Unorm {
                        rgb.extend([p[2], p[1], p[0]]);
                    } else {
                        rgb.extend_from_slice(&p[..3]);
                    }
                }
            }
            Ok(rgb)
        })();
        staging.unmap();
        result
    }
}

pub struct WindowPresenter {
    renderer: &'static Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    compositor: Compositor,
    configured: bool,
}

impl Renderer {
    pub fn create_window_presenter(
        &'static self,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
    ) -> Result<WindowPresenter, String> {
        let surface = self.instance.create_surface(target).map_err(|e| e.to_string())?;
        let caps = surface.get_capabilities(&self.adapter);
        // The CPU presenter writes sRGB byte values directly. Require a linear
        // UNORM swapchain to preserve those bytes without a second gamma encode.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .ok_or("Vulkan surface has no byte-preserving UNORM format")?;
        let mut config = surface.get_default_config(&self.adapter, 1, 1).ok_or("Vulkan adapter cannot present to this window")?;
        config.format = format;
        config.present_mode = wgpu::PresentMode::AutoNoVsync;
        let compositor = Compositor::new(self, format);
        Ok(WindowPresenter { renderer: self, surface, config, compositor, configured: false })
    }

    /// An offscreen diagnostic of the exact window compositor, without a surface.
    pub fn compose_display_rgb(&self, width: u32, height: u32, screens: &[DisplayScreen<'_>]) -> Result<Vec<u8>, String> {
        Compositor::new(self, wgpu::TextureFormat::Rgba8Unorm).capture(self, width, height, screens)
    }
}

impl WindowPresenter {
    pub fn present(&mut self, width: u32, height: u32, screens: &[DisplayScreen<'_>]) -> Result<bool, String> {
        validate_screens(self.renderer, screens)?;
        if width == 0 || height == 0 {
            return Ok(false);
        }
        if width > self.renderer.device.limits().max_texture_dimension_2d
            || height > self.renderer.device.limits().max_texture_dimension_2d
        {
            return Err("window dimensions exceed the Vulkan device limit".into());
        }
        if !self.configured || self.config.width != width || self.config.height != height {
            self.config.width = width;
            self.config.height = height;
            let _guard = self.renderer.submission_lock.lock().unwrap_or_else(|p| p.into_inner());
            self.surface.configure(&self.renderer.device, &self.config);
            self.configured = true;
        }
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return Ok(false),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.configured = false;
                return Ok(false);
            }
            other => return Err(format!("cannot acquire Vulkan window image: {other:?}")),
        };
        let mut encoder = self.renderer.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.compositor.encode(
            self.renderer,
            &mut encoder,
            &output.texture.create_view(&Default::default()),
            width,
            height,
            screens,
        );
        // Present and submit share the configure exclusion with the game thread.
        let _guard = self.renderer.submission_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.renderer.queue.submit([encoder.finish()]);
        self.renderer.queue.present(output);
        Ok(true)
    }

    pub fn capture_rgb(&self, width: u32, height: u32, screens: &[DisplayScreen<'_>]) -> Result<Vec<u8>, String> {
        self.compositor.capture(self.renderer, width, height, screens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type ReferenceScreen<'a> = (&'a [u8], (u32, u32), [u32; 4]);

    fn reference(width: u32, height: u32, screens: &[ReferenceScreen<'_>]) -> Vec<u8> {
        let mut rgb = vec![0; width as usize * height as usize * 3];
        for &(pixels, (sw, sh), [left, top, dw, dh]) in screens {
            if left >= width || top >= height {
                continue;
            }
            for dy in 0..dh.min(height - top) {
                for dx in 0..dw.min(width - left) {
                    let sx = u64::from(dx) * u64::from(sw) / u64::from(dw);
                    let sy = u64::from(dy) * u64::from(sh) / u64::from(dh);
                    let source = (sy * u64::from(sw) + sx) as usize * 3;
                    let at = ((top + dy) * width + left + dx) as usize * 3;
                    rgb[at..at + 3].copy_from_slice(&pixels[source..source + 3]);
                }
            }
        }
        rgb
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn resident_compositor_matches_cpu_sampling_clipping_channels_and_does_not_read_sources() {
        let renderer = Renderer::new().unwrap();
        let top: Vec<_> = (0..13 * 29 * 3).map(|n| (n * 73 + 17) as u8).collect();
        let bottom: Vec<_> = (0..7 * 5 * 3).map(|n| (n * 37 + 255) as u8).collect();
        let a = renderer.upload_display_rgb(13, 29, &top).unwrap();
        let b = renderer.upload_display_rgb(7, 5, &bottom).unwrap();
        for format in [wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Bgra8Unorm] {
            let compositor = Compositor::new(&renderer, format);
            for (width, height) in [(1, 1), (23, 19), (47, 35), (29, 17), (800, 960)] {
                for rects in [
                    [[2, 1, 17, 13], [6, 8, 19, 7]],
                    [[0, 0, 400, 480], [40, 480, 320, 480]],
                    [[width + 1, 0, 7, 9], [0, height + 1, 0, 0]],
                ] {
                    let screens = [DisplayScreen { frame: &a, rect: rects[0] }, DisplayScreen { frame: &b, rect: rects[1] }];
                    let actual = compositor.capture(&renderer, width, height, &screens).unwrap();
                    assert_eq!(actual, reference(width, height, &[(&top, (13, 29), rects[0]), (&bottom, (7, 5), rects[1])]));
                    assert!(!a.ready() && !b.ready(), "presentation never maps source buffers");
                }
            }
        }
        assert!(renderer.compose_display_rgb(10, 10, &[DisplayScreen { frame: &a, rect: [u32::MAX, 0, 1, 1] }]).is_err());
        let second = Renderer::new().unwrap();
        assert!(second.compose_display_rgb(10, 10, &[DisplayScreen { frame: &a, rect: [0, 0, 10, 10] }]).is_err());
        let first = a.read_rgb().unwrap();
        assert_eq!(first.as_ref(), top);
        assert!(std::sync::Arc::ptr_eq(first, a.read_rgb().unwrap()));
    }
}

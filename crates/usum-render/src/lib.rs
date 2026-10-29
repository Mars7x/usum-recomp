//! Host-GPU renderer for PICA200 command streams.
//!
//! The current path owns a Vulkan-only wgpu device and accepts the output of
//! PICA vertex processing. It provides batched persistent color/depth targets,
//! raster state, textures, TEV, fragment lighting, framebuffer snapshot upload
//! and readback. The runtime handoff remains; the software renderer stays
//! active until output can be compared draw-for-draw.

mod raster;
mod vertex;
pub use vertex::{PicaVertexDraw, VertexProgram};

pub use raster::{
    AlphaTest, BlendComponent, BlendFactor, BlendOperation, BlendState, ColorDepthReadback, ColorMask, ColorTarget, CullMode,
    DepthCompare, DepthSnapshot, DepthState, DisplayCopy, FilterMode, FragmentInputs, GpuLighting, LightingLight, LightingState,
    PendingReadback, PicaDrawState, PicaRasterRegisters, RasterBatch, RasterState, RasterVertex, SampledTexture, SamplerState,
    StencilOperation, StencilState, TevStage, TevState, Viewport, WrapMode,
};

/// A Vulkan device ready to host translated PICA draw calls.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: wgpu::AdapterInfo,
    raster_pipelines: raster::RasterPipelines,
}

impl Renderer {
    /// Opens a hardware Vulkan adapter without needing a window or surface.
    pub fn new() -> Result<Self, String> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no Vulkan adapter: {e}"))?;
        let info = adapter.get_info();
        let required = wgpu::Features::DEPTH32FLOAT_STENCIL8;
        if !adapter.features().contains(required) {
            return Err(format!("Vulkan adapter {} cannot preserve float depth with stencil", info.name));
        }
        let optional_features = required | (adapter.features() & wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("usum-render"),
            required_features: optional_features,
            ..Default::default()
        }))
        .map_err(|e| format!("cannot open Vulkan device {}: {e}", info.name))?;
        let raster_pipelines = raster::create_pipelines(&device, &queue);
        Ok(Self { device, queue, info, raster_pipelines })
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.info
    }

    /// Compiles and submits the smallest complete graphics pipeline to an
    /// off-screen target, reads it back and checks a pixel inside the
    /// triangle. This is not a software framebuffer upload; Vulkan performs
    /// rasterization and color interpolation.
    pub fn smoke_triangle(&self) -> Result<(), String> {
        let target = self.create_color_target(16, 16)?;
        self.draw_vertices(
            &target,
            &[
                RasterVertex::new([-0.75, -0.75, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0]),
                RasterVertex::new([0.75, -0.75, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]),
                RasterVertex::new([0.0, 0.75, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]),
            ],
        )?;
        // A separate submission in one corner must be added to, rather than
        // clearing, the first triangle.
        self.draw_vertices(
            &target,
            &[
                RasterVertex::new([0.5, 0.5, 0.0, 1.0], [1.0; 4]),
                RasterVertex::new([0.9, 0.5, 0.0, 1.0], [1.0; 4]),
                RasterVertex::new([0.7, 0.9, 0.0, 1.0], [1.0; 4]),
            ],
        )?;
        let image = self.read_color_target(&target)?;
        let pixel = &image[(8 * 16 + 8) * 4..][..4];
        // At the target centre the expected barycentric weights are about
        // 1/4 red, 1/4 green and 1/2 blue. Leave room for pixel-centre and
        // UNORM rounding while requiring every channel to be interpolated.
        if !(40..=90).contains(&pixel[0]) || !(40..=90).contains(&pixel[1]) || !(105..=150).contains(&pixel[2]) || pixel[3] != 255
        {
            return Err(format!("Vulkan smoke draw returned centre pixel {pixel:?}"));
        }
        let white = image.as_chunks::<4>().0.iter().filter(|p| p[0] > 240 && p[1] > 240 && p[2] > 240 && p[3] == 255).count();
        if white == 0 {
            return Err("Vulkan second draw did not appear in the persistent target".into());
        }

        let depth_target = self.create_color_target(16, 16)?;
        let triangle = |z, color| {
            [
                RasterVertex::new([-0.75, -0.75, z, 1.0], color),
                RasterVertex::new([0.75, -0.75, z, 1.0], color),
                RasterVertex::new([0.0, 0.75, z, 1.0], color),
            ]
        };
        let batch_target = self.create_color_target(16, 16)?;
        let mut batch = self.begin_batch(&batch_target);
        batch.draw(&triangle(-0.75, [0.0, 0.0, 1.0, 1.0]), Viewport::full(&batch_target), RasterState::default())?;
        batch.draw(&triangle(-0.25, [1.0, 1.0, 0.0, 1.0]), Viewport::full(&batch_target), RasterState::default())?;
        let (batch_image, batch_depth) = batch.finish_and_read_color_and_depth()?;
        let batch_pixel = &batch_image[(8 * 16 + 8) * 4..][..4];
        if batch_pixel[0] < 240 || batch_pixel[1] < 240 || batch_pixel[2] > 15 || batch_pixel[3] != 255 {
            return Err(format!("Vulkan batched depth ordering returned centre pixel {batch_pixel:?}"));
        }
        if (batch_depth[8 * 16 + 8] - 0.25).abs() > 1e-6 {
            return Err(format!("Vulkan batched readback returned depth {}, expected 0.25", batch_depth[8 * 16 + 8]));
        }

        self.draw_vertices(&depth_target, &triangle(-0.75, [0.0, 0.0, 1.0, 1.0]))?;
        self.draw_vertices(&depth_target, &triangle(-0.25, [1.0, 1.0, 0.0, 1.0]))?;
        // Submitted last but farther away: it must fail the depth test.
        self.draw_vertices(&depth_target, &triangle(-0.9, [1.0, 0.0, 1.0, 1.0]))?;
        let depth_image = self.read_color_target(&depth_target)?;
        let depth_pixel = &depth_image[(8 * 16 + 8) * 4..][..4];
        if depth_pixel[0] < 240 || depth_pixel[1] < 240 || depth_pixel[2] > 15 || depth_pixel[3] != 255 {
            return Err(format!("Vulkan depth ordering returned centre pixel {depth_pixel:?}"));
        }

        // A runtime mirror starts from the guest's current color and depth,
        // not a clear target. A farther draw must preserve both snapshots;
        // a nearer draw must then replace them.
        let snapshot_color = [255, 0, 255, 255].repeat(8 * 8);
        let snapshot_depth = vec![0.25; 8 * 8];
        let snapshot_target = self.create_color_target_from_snapshot(8, 8, &snapshot_color, &snapshot_depth)?;
        self.draw_vertices(&snapshot_target, &triangle(-0.75, [0.0, 0.0, 1.0, 1.0]))?;
        let preserved = self.read_color_target(&snapshot_target)?;
        if preserved[(4 * 8 + 4) * 4..][..4] != [255, 0, 255, 255] {
            return Err("Vulkan target upload did not preserve guest color/depth against a farther draw".into());
        }
        self.draw_vertices(&snapshot_target, &triangle(-0.1, [0.0, 1.0, 0.0, 1.0]))?;
        let (replaced, replaced_depth) = self.read_color_and_depth_target(&snapshot_target)?;
        if replaced[(4 * 8 + 4) * 4..][..4] != [0, 255, 0, 255] {
            return Err("Vulkan target upload did not accept a nearer draw".into());
        }
        let centre_depth = replaced_depth[4 * 8 + 4];
        if (centre_depth - 0.1).abs() > 1e-6 {
            return Err(format!("Vulkan depth readback returned {centre_depth}, expected 0.1"));
        }
        let mut stencil_batch = self.begin_batch(&snapshot_target);
        stencil_batch.seed_stencil(&[0xA5; 8 * 8], 1)?;
        stencil_batch.submit()?;
        let feedback = self.create_d24s8_feedback_texture(&snapshot_target, SamplerState::default(), [0; 4])?;
        let feedback_target = self.create_color_target(8, 8)?;
        let feedback_vertices = [
            RasterVertex::new([-1.0, -1.0, 0.0, 1.0], [1.0; 4]).with_texcoord0([0.0, 0.0]),
            RasterVertex::new([1.0, -1.0, 0.0, 1.0], [1.0; 4]).with_texcoord0([1.0, 0.0]),
            RasterVertex::new([1.0, 1.0, 0.0, 1.0], [1.0; 4]).with_texcoord0([1.0, 1.0]),
            RasterVertex::new([-1.0, -1.0, 0.0, 1.0], [1.0; 4]).with_texcoord0([0.0, 0.0]),
            RasterVertex::new([1.0, 1.0, 0.0, 1.0], [1.0; 4]).with_texcoord0([1.0, 1.0]),
            RasterVertex::new([-1.0, 1.0, 0.0, 1.0], [1.0; 4]).with_texcoord0([0.0, 1.0]),
        ];
        self.draw_vertices_with_inputs(
            &feedback_target,
            &feedback_vertices,
            Viewport::full(&feedback_target),
            RasterState { tev: TevState::modulate_texture0(), ..RasterState::default() },
            FragmentInputs { texture0: Some(&feedback), ..FragmentInputs::default() },
        )?;
        let feedback_image = self.read_color_target(&feedback_target)?;
        let feedback_pixel = &feedback_image[(4 * 8 + 4) * 4..][..4];
        if feedback_pixel[0] != 0xA5
            || feedback_pixel[1] != 0x19
            || feedback_pixel[2].abs_diff(0x99) > 1
            || feedback_pixel[3].abs_diff(0x99) > 1
        {
            return Err(format!("Vulkan D24S8 feedback conversion returned {feedback_pixel:?}"));
        }

        // Exercise every texel with varying stencil and exactly representable
        // depths, so row orientation and all four channels are checked.
        let pattern_depth: Vec<f32> = (0..64).map(|i| [0.0, 0.25, 0.5, 1.0][i % 4]).collect();
        let pattern_stencil: Vec<u8> = (0..64).map(|i| (i * 3) as u8).collect();
        let pattern_target = self.create_color_target(8, 8)?;
        // No CPU wait between the depth upload, conversion, and sampling.
        // The final readback must observe ordered queue execution.
        self.begin_batch_from_snapshot(&pattern_target, &snapshot_color, &pattern_depth)?.submit()?;
        let mut stencil_batch = self.begin_batch(&pattern_target);
        stencil_batch.seed_stencil(&pattern_stencil, 1)?;
        stencil_batch.submit()?;
        let pattern_texture = self.create_d24s8_feedback_texture(&pattern_target, SamplerState::default(), [0; 4])?;
        let pattern_output = self.create_color_target(8, 8)?;
        self.draw_vertices_with_inputs(
            &pattern_output,
            &feedback_vertices,
            Viewport::full(&pattern_output),
            RasterState { tev: TevState::modulate_texture0(), ..RasterState::default() },
            FragmentInputs { texture0: Some(&pattern_texture), ..FragmentInputs::default() },
        )?;
        let pattern_image = self.read_color_target(&pattern_output)?;
        for (i, pixel) in pattern_image.as_chunks::<4>().0.iter().enumerate() {
            let depth = (pattern_depth[i] * 16_777_215.0) as u32;
            let expected = [pattern_stencil[i], (depth >> 16) as u8, (depth >> 8) as u8, depth as u8];
            if *pixel != expected {
                return Err(format!("Vulkan feedback texel {i}: {pixel:?}, expected {expected:?}"));
            }
        }

        // Equal-depth passes must compare in the same integer domain as the
        // guest buffer, rather than comparing the unquantized float to it.
        for maximum in [0xFFFF, 0xFF_FFFF] {
            for incoming in [0.0_f32, 1.0 / maximum as f32, 0.125, 0.25, 0.5, 1.0] {
                let quantized = (incoming * maximum as f32).floor() / maximum as f32;
                let equal_target = self.create_color_target_from_snapshot(8, 8, &snapshot_color, &[quantized; 64])?;
                self.draw_vertices_with_raster_state(
                    &equal_target,
                    &triangle(-incoming, [0.0, 1.0, 0.0, 1.0]),
                    Viewport::full(&equal_target),
                    RasterState {
                        depth: DepthState { compare: DepthCompare::Equal, write: true },
                        depth_max: maximum,
                        ..RasterState::default()
                    },
                )?;
                let (color, depth) = self.read_color_and_depth_target(&equal_target)?;
                if color[(4 * 8 + 4) * 4..][..4] != [0, 255, 0, 255] || depth[4 * 8 + 4] != quantized {
                    return Err(format!("Vulkan integer depth equality failed for range {maximum:#x} at {incoming}"));
                }
            }
        }

        // A disabled test passes color, but with writes disabled it must leave
        // the previous depth intact for a following comparison.
        let depth_state_target = self.create_color_target(16, 16)?;
        self.draw_vertices(&depth_state_target, &triangle(-0.25, [1.0, 1.0, 0.0, 1.0]))?;
        self.draw_vertices_with_state(
            &depth_state_target,
            &triangle(-0.75, [1.0, 0.0, 1.0, 1.0]),
            Viewport::full(&depth_state_target),
            CullMode::None,
            DepthState { compare: DepthCompare::Always, write: false },
        )?;
        self.draw_vertices_with_state(
            &depth_state_target,
            &triangle(-0.5, [0.0, 1.0, 1.0, 1.0]),
            Viewport::full(&depth_state_target),
            CullMode::None,
            DepthState { compare: DepthCompare::Less, write: true },
        )?;
        let depth_state_image = self.read_color_target(&depth_state_target)?;
        let state_pixel = &depth_state_image[(8 * 16 + 8) * 4..][..4];
        if state_pixel != [255, 0, 255, 255] {
            return Err(format!("Vulkan depth compare/write state returned centre pixel {state_pixel:?}"));
        }

        let color_mask_target = self.create_color_target(16, 16)?;
        self.draw_vertices_with_color_mask(
            &color_mask_target,
            &triangle(-0.5, [1.0; 4]),
            Viewport::full(&color_mask_target),
            CullMode::None,
            DepthState { compare: DepthCompare::Always, write: false },
            ColorMask::from_bits(0b0101),
        )?;
        let color_mask_image = self.read_color_target(&color_mask_target)?;
        let mask_pixel = &color_mask_image[(8 * 16 + 8) * 4..][..4];
        if mask_pixel != [255, 0, 255, 0] {
            return Err(format!("Vulkan color write mask returned centre pixel {mask_pixel:?}"));
        }

        let blend_target = self.create_color_target(16, 16)?;
        self.draw_vertices(&blend_target, &triangle(-0.5, [0.0, 0.0, 1.0, 1.0]))?;
        let blend_functions = 6 << 16 | 7 << 20 | 1 << 24;
        let blend = BlendState::from_pica(1 << 8, blend_functions, 0).ok_or("blend state did not enable")?;
        self.draw_vertices_with_raster_state(
            &blend_target,
            &triangle(-0.5, [1.0, 0.0, 0.0, 0.5]),
            Viewport::full(&blend_target),
            RasterState {
                cull: CullMode::None,
                depth_max: 0,
                depth: DepthState { compare: DepthCompare::Always, write: false },
                color_mask: ColorMask::ALL,
                blend: Some(blend),
                alpha_test: None,
                tev: TevState::default(),
                ..RasterState::default()
            },
        )?;
        let blend_image = self.read_color_target(&blend_target)?;
        let blend_pixel = &blend_image[(8 * 16 + 8) * 4..][..4];
        if !(120..=135).contains(&blend_pixel[0])
            || blend_pixel[1] != 0
            || !(120..=135).contains(&blend_pixel[2])
            || !(120..=135).contains(&blend_pixel[3])
        {
            return Err(format!("Vulkan alpha blend returned centre pixel {blend_pixel:?}"));
        }

        let alpha_target = self.create_color_target(16, 16)?;
        let alpha_test = AlphaTest::from_pica(1 | 6 << 4 | 128 << 8).ok_or("alpha test did not enable")?;
        let alpha_state = RasterState {
            depth: DepthState { compare: DepthCompare::Always, write: false },
            alpha_test: Some(alpha_test),
            ..RasterState::default()
        };
        self.draw_vertices_with_raster_state(
            &alpha_target,
            &triangle(-0.5, [0.0, 1.0, 0.0, 0.25]),
            Viewport::full(&alpha_target),
            alpha_state,
        )?;
        let rejected = self.read_color_target(&alpha_target)?;
        if rejected[(8 * 16 + 8) * 4..][..4] != [0, 0, 0, 0] {
            return Err("Vulkan alpha test accepted a fragment below the reference".into());
        }
        self.draw_vertices_with_raster_state(
            &alpha_target,
            &triangle(-0.5, [0.0, 1.0, 0.0, 0.75]),
            Viewport::full(&alpha_target),
            alpha_state,
        )?;
        let accepted = self.read_color_target(&alpha_target)?;
        let alpha_pixel = &accepted[(8 * 16 + 8) * 4..][..4];
        if alpha_pixel[0] != 0 || alpha_pixel[1] != 255 || alpha_pixel[2] != 0 || !(185..=195).contains(&alpha_pixel[3]) {
            return Err(format!("Vulkan alpha test rejected or changed an accepted fragment: {alpha_pixel:?}"));
        }

        let texels = [
            255, 0, 0, 255, 0, 255, 0, 255, // first row: red, green
            0, 0, 255, 255, 255, 255, 255, 255, // second row: blue, white
        ];
        let repeated_texture = self.create_rgba8_texture(
            2,
            2,
            &texels,
            SamplerState { wrap_u: WrapMode::Repeat, wrap_v: WrapMode::ClampToEdge, filter: FilterMode::Nearest },
        )?;
        let texture_target = self.create_color_target(16, 16)?;
        let repeated_vertices = triangle(-0.5, [1.0; 4]).map(|vertex| vertex.with_texcoord0([1.25, 0.25]));
        self.draw_vertices_with_inputs(
            &texture_target,
            &repeated_vertices,
            Viewport::full(&texture_target),
            RasterState { tev: TevState::modulate_texture0(), ..RasterState::default() },
            FragmentInputs { texture0: Some(&repeated_texture), ..FragmentInputs::default() },
        )?;
        let texture_image = self.read_color_target(&texture_target)?;
        let texture_pixel = &texture_image[(8 * 16 + 8) * 4..][..4];
        if texture_pixel != [255, 0, 0, 255] {
            return Err(format!("Vulkan repeated nearest texture returned centre pixel {texture_pixel:?}"));
        }

        let linear_texture =
            self.create_rgba8_texture(2, 2, &texels, SamplerState { filter: FilterMode::Linear, ..SamplerState::default() })?;
        let linear_target = self.create_color_target(16, 16)?;
        let linear_vertices = triangle(-0.5, [1.0; 4]).map(|vertex| vertex.with_texcoord0([0.5, 0.5]));
        self.draw_vertices_with_inputs(
            &linear_target,
            &linear_vertices,
            Viewport::full(&linear_target),
            RasterState { tev: TevState::modulate_texture0(), ..RasterState::default() },
            FragmentInputs { texture0: Some(&linear_texture), ..FragmentInputs::default() },
        )?;
        let linear_image = self.read_color_target(&linear_target)?;
        let linear_pixel = &linear_image[(8 * 16 + 8) * 4..][..4];
        if linear_pixel[..3].iter().any(|channel| !(120..=135).contains(channel)) || linear_pixel[3] != 255 {
            return Err(format!("Vulkan linear texture returned centre pixel {linear_pixel:?}"));
        }

        let mut chained_tev = TevState::modulate_texture0();
        chained_tev.stages[1] =
            TevStage { sources: 15 | 14 << 4 | 15 << 16, operands: 0, combiner: 2, constant: 0x0000_4000, scale: 0 };
        chained_tev.stages[2] = TevStage { scale: 1, ..TevStage::replace(15) };
        let tev_target = self.create_color_target(16, 16)?;
        let tev_vertices = triangle(-0.5, [0.25, 0.25, 0.25, 1.0]).map(|vertex| vertex.with_texcoord0([0.25, 0.25]));
        self.draw_vertices_with_inputs(
            &tev_target,
            &tev_vertices,
            Viewport::full(&tev_target),
            RasterState { tev: chained_tev, ..RasterState::default() },
            FragmentInputs { texture0: Some(&repeated_texture), ..FragmentInputs::default() },
        )?;
        let tev_image = self.read_color_target(&tev_target)?;
        let tev_pixel = &tev_image[(8 * 16 + 8) * 4..][..4];
        if !(120..=135).contains(&tev_pixel[0])
            || !(120..=135).contains(&tev_pixel[1])
            || tev_pixel[2] != 0
            || tev_pixel[3] != 255
        {
            return Err(format!("Vulkan chained TEV stages returned centre pixel {tev_pixel:?}"));
        }

        // Buffer writes take effect after the current stage reads its inputs.
        // Stage 1 must see the initial green buffer; stage 2 sees stage 0 red.
        for (read_stage, expected) in [(1, [0, 255, 0, 64]), (2, [255, 0, 0, 191])] {
            let mut tev = TevState { buffer_color: 0x4000_FF00, buffer_update: (1 << 8) | (1 << 12), ..TevState::default() };
            tev.stages[read_stage] = TevStage::replace(13);
            let target = self.create_color_target(8, 8)?;
            self.draw_vertices_with_raster_state(
                &target,
                &triangle(-0.5, [1.0, 0.0, 0.0, 0.75]),
                Viewport::full(&target),
                RasterState { tev, ..RasterState::default() },
            )?;
            let image = self.read_color_target(&target)?;
            let pixel = &image[(4 * 8 + 4) * 4..][..4];
            if pixel.iter().zip(expected).any(|(&actual, expected)| actual.abs_diff(expected) > 1) {
                return Err(format!("Vulkan TEV buffer stage {read_stage}: {pixel:?}, expected {expected:?}"));
            }
        }

        for (unit, source, coordinate, expected) in
            [(1, 4, [0.75, 0.25], [0, 255, 0, 255]), (2, 5, [0.25, 0.75], [0, 0, 255, 255])]
        {
            let mut tev = TevState::default();
            tev.stages[0] = TevStage::replace(source);
            let unit_target = self.create_color_target(16, 16)?;
            let unit_vertices = triangle(-0.5, [1.0; 4]).map(|vertex| {
                if unit == 1 {
                    vertex.with_texcoord1(coordinate)
                } else {
                    vertex.with_texcoord2(coordinate)
                }
            });
            let inputs = if unit == 1 {
                FragmentInputs { texture1: Some(&repeated_texture), ..FragmentInputs::default() }
            } else {
                FragmentInputs { texture2: Some(&repeated_texture), ..FragmentInputs::default() }
            };
            self.draw_vertices_with_inputs(
                &unit_target,
                &unit_vertices,
                Viewport::full(&unit_target),
                RasterState { tev, ..RasterState::default() },
                inputs,
            )?;
            let unit_image = self.read_color_target(&unit_target)?;
            let unit_pixel = &unit_image[(8 * 16 + 8) * 4..][..4];
            if unit_pixel != expected {
                return Err(format!("Vulkan texture unit {unit} returned centre pixel {unit_pixel:?}"));
            }
        }

        let white_directional = LightingLight {
            id: 0,
            specular0: [0.0; 3],
            specular1: [0.0; 3],
            diffuse: [1.0; 3],
            ambient: [0.0; 3],
            position: [0.0, 0.0, 1.0],
            spot_direction: [0.0; 3],
            directional: true,
            two_sided: false,
            geometric: [false; 2],
            attenuation_bias: 0.0,
            attenuation_scale: 0.0,
        };
        let lighting = self.create_lighting(&LightingState {
            lights: vec![white_directional],
            ambient: [0.0; 3],
            config0: 0,
            config1: u32::MAX,
            abs: 0,
            select: 0,
            scale: 0,
            luts: vec![[0; 256]; 24].into(),
        })?;
        let mut lighting_tev = TevState::default();
        lighting_tev.stages[0] = TevStage::replace(1);
        for (quaternion, expected, label) in
            [([0.0, 0.0, 0.0, 1.0], [255, 255, 255, 255], "facing"), ([1.0, 0.0, 0.0, 0.0], [0, 0, 0, 255], "away")]
        {
            let lighting_target = self.create_color_target(16, 16)?;
            let lighting_vertices =
                triangle(-0.5, [0.0; 4]).map(|vertex| vertex.with_lighting_vectors(quaternion, [0.0, 0.0, 1.0]));
            self.draw_vertices_with_inputs(
                &lighting_target,
                &lighting_vertices,
                Viewport::full(&lighting_target),
                RasterState { tev: lighting_tev, ..RasterState::default() },
                FragmentInputs { lighting: Some(&lighting), ..FragmentInputs::default() },
            )?;
            let lighting_image = self.read_color_target(&lighting_target)?;
            let lighting_pixel = &lighting_image[(8 * 16 + 8) * 4..][..4];
            if lighting_pixel != expected {
                return Err(format!("Vulkan {label} directional lighting returned centre pixel {lighting_pixel:?}"));
            }
        }

        let mut distribution_luts = vec![[0; 256]; 24];
        distribution_luts[0][255] = 1024 | 512 << 12;
        let distribution_lighting = self.create_lighting(&LightingState {
            lights: vec![LightingLight { diffuse: [0.0; 3], specular0: [1.0; 3], ..white_directional }],
            ambient: [0.0; 3],
            config0: 0,
            config1: !(1 << 16),
            abs: 0,
            select: 0,
            scale: 0,
            luts: distribution_luts.into(),
        })?;
        let mut distribution_tev = TevState::default();
        distribution_tev.stages[0] = TevStage::replace(2);
        let distribution_target = self.create_color_target(16, 16)?;
        self.draw_vertices_with_inputs(
            &distribution_target,
            &triangle(-0.5, [0.0; 4]),
            Viewport::full(&distribution_target),
            RasterState { tev: distribution_tev, ..RasterState::default() },
            FragmentInputs { lighting: Some(&distribution_lighting), ..FragmentInputs::default() },
        )?;
        let distribution_image = self.read_color_target(&distribution_target)?;
        let distribution_pixel = &distribution_image[(8 * 16 + 8) * 4..][..4];
        if distribution_pixel[..3].iter().any(|channel| !(120..=135).contains(channel)) || distribution_pixel[3] != 255 {
            return Err(format!("Vulkan D0 lighting LUT returned centre pixel {distribution_pixel:?}"));
        }

        let viewport_target = self.create_color_target(16, 16)?;
        self.draw_vertices_in(&viewport_target, &triangle(-0.5, [1.0; 4]), Viewport { x: 4.0, y: 4.0, width: 8.0, height: 8.0 })?;
        let viewport_image = self.read_color_target(&viewport_target)?;
        let centre = &viewport_image[(8 * 16 + 8) * 4..][..4];
        let corner = &viewport_image[0..4];
        if centre != [255, 255, 255, 255] || corner != [0, 0, 0, 0] {
            return Err(format!("Vulkan viewport returned centre {centre:?}, corner {corner:?}"));
        }

        let back_cull = self.create_color_target(16, 16)?;
        self.draw_vertices_with(&back_cull, &triangle(-0.5, [1.0; 4]), Viewport::full(&back_cull), CullMode::Back)?;
        let back_pixel = self.read_color_target(&back_cull)?;
        let front_cull = self.create_color_target(16, 16)?;
        self.draw_vertices_with(&front_cull, &triangle(-0.5, [1.0; 4]), Viewport::full(&front_cull), CullMode::Front)?;
        let front_pixel = self.read_color_target(&front_cull)?;
        if back_pixel[(8 * 16 + 8) * 4..][..4] != [255, 255, 255, 255] || front_pixel[(8 * 16 + 8) * 4..][..4] != [0, 0, 0, 0] {
            return Err("Vulkan front/back culling does not match PICA's counter-clockwise front face".into());
        }
        Ok(())
    }
}

use super::*;

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum StencilOperation {
    #[default]
    Keep,
    Zero,
    Replace,
    IncrementClamp,
    DecrementClamp,
    Invert,
    IncrementWrap,
    DecrementWrap,
}

impl StencilOperation {
    fn from_pica(value: u32) -> Self {
        match value & 7 {
            0 => Self::Keep,
            1 => Self::Zero,
            2 => Self::Replace,
            3 => Self::IncrementClamp,
            4 => Self::DecrementClamp,
            5 => Self::Invert,
            6 => Self::IncrementWrap,
            _ => Self::DecrementWrap,
        }
    }
    fn wgpu(self) -> wgpu::StencilOperation {
        match self {
            Self::Keep => wgpu::StencilOperation::Keep,
            Self::Zero => wgpu::StencilOperation::Zero,
            Self::Replace => wgpu::StencilOperation::Replace,
            Self::IncrementClamp => wgpu::StencilOperation::IncrementClamp,
            Self::DecrementClamp => wgpu::StencilOperation::DecrementClamp,
            Self::Invert => wgpu::StencilOperation::Invert,
            Self::IncrementWrap => wgpu::StencilOperation::IncrementWrap,
            Self::DecrementWrap => wgpu::StencilOperation::DecrementWrap,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct StencilState {
    pub enabled: bool,
    pub compare: DepthCompare,
    pub read_mask: u8,
    pub write_mask: u8,
    pub reference: u8,
    pub fail: StencilOperation,
    pub depth_fail: StencilOperation,
    pub pass: StencilOperation,
}

impl Default for StencilState {
    fn default() -> Self {
        Self {
            enabled: false,
            compare: DepthCompare::Always,
            read_mask: 255,
            write_mask: 0,
            reference: 0,
            fail: StencilOperation::Keep,
            depth_fail: StencilOperation::Keep,
            pass: StencilOperation::Keep,
        }
    }
}

impl StencilState {
    pub fn from_pica(test: u32, operations: u32, permissions: u32, format: u32) -> Self {
        Self {
            enabled: format & 3 == 3 && test & 1 != 0,
            compare: DepthCompare::from_pica(test >> 4),
            read_mask: (test >> 24) as u8,
            write_mask: if permissions & 1 != 0 { (test >> 8) as u8 } else { 0 },
            reference: (test >> 16) as u8,
            fail: StencilOperation::from_pica(operations),
            depth_fail: StencilOperation::from_pica(operations >> 4),
            pass: StencilOperation::from_pica(operations >> 8),
        }
    }

    pub fn writes(self) -> bool {
        self.enabled
            && self.write_mask != 0
            && [self.fail, self.depth_fail, self.pass].iter().any(|op| *op != StencilOperation::Keep)
    }

    pub(super) fn key(self) -> Self {
        if self.enabled {
            Self { reference: 0, ..self }
        } else {
            Self::default()
        }
    }

    pub(super) fn wgpu(self) -> wgpu::StencilState {
        if !self.enabled {
            return wgpu::StencilState::default();
        }
        let face = wgpu::StencilFaceState {
            compare: self.compare.wgpu(),
            fail_op: self.fail.wgpu(),
            depth_fail_op: self.depth_fail.wgpu(),
            pass_op: self.pass.wgpu(),
        };
        wgpu::StencilState {
            front: face,
            back: face,
            read_mask: u32::from(self.read_mask),
            write_mask: u32::from(self.write_mask),
        }
    }
}

impl RasterBatch<'_> {
    /// Import native stencil bytes without changing color or float depth.
    /// Uniform clears stay on the GPU; other seeds retain nearest sampling.
    pub fn seed_stencil(&mut self, native: &[u8], scale: u32) -> Result<(), String> {
        let (width, height) = self.target.dimensions();
        if scale == 0
            || !width.is_multiple_of(scale)
            || !height.is_multiple_of(scale)
            || native.len() != (width / scale) as usize * (height / scale) as usize
        {
            return Err("stencil seed dimensions do not match target".into());
        }
        let encoder = self.encoder.as_mut().ok_or("Vulkan raster batch was already submitted")?;
        let first = native[0];
        if native.iter().all(|&v| v == first) {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native stencil clear"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.target.depth_view,
                    depth_ops: None,
                    stencil_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(u32::from(first)),
                        store: wgpu::StoreOp::Store,
                    }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        } else {
            use wgpu::util::DeviceExt;
            let padded_row = width.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let mut bytes = vec![0; (padded_row * height) as usize];
            for y in 0..height {
                for x in 0..width {
                    bytes[(y * padded_row + x) as usize] = native[((y / scale) * (width / scale) + x / scale) as usize];
                }
            }
            let upload = self.renderer.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native stencil upload"),
                contents: &bytes,
                usage: wgpu::BufferUsages::COPY_SRC,
            });
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &upload,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.target.depth,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::StencilOnly,
                },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> [RasterVertex; 6] {
        let v = |x, y| RasterVertex::new([x, y, -0.5, 1.0], [1.0; 4]);
        [v(-1.0, -1.0), v(1.0, -1.0), v(1.0, 1.0), v(-1.0, -1.0), v(1.0, 1.0), v(-1.0, 1.0)]
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn stencil_seeds_resolve_snapshots_and_dynamic_references_are_exact() {
        let renderer = Renderer::new().unwrap();
        for scale in [1, 2, 3, 6, 8] {
            let target = renderer.create_color_target(31 * scale, 19 * scale).unwrap();
            let seed: Vec<u8> = (0..31 * 19).map(|i| (i * 71) as u8).collect();
            let mut batch = renderer.begin_batch(&target);
            batch.seed_stencil(&seed, scale).unwrap();
            let ticket = batch.submit_resolved_readback(scale).unwrap();
            assert_eq!(ticket.read().unwrap().stencil, seed);
            let snapshot = renderer.snapshot_depth(&target).unwrap();
            let mut batch = renderer.begin_batch(&target);
            batch.seed_stencil(&[255; 31 * 19], scale).unwrap();
            batch.submit().unwrap();
            let restored = renderer.create_color_target(31 * scale, 19 * scale).unwrap();
            let mut batch = renderer.begin_batch(&restored);
            batch.restore_depth(&snapshot).unwrap();
            let restored = batch.submit_resolved_readback(scale).unwrap();
            assert_eq!(restored.read().unwrap().stencil, seed);
            assert_eq!(ticket.read().unwrap().stencil, seed);
        }
        let target = renderer.create_color_target(8, 8).unwrap();
        for reference in [0x91, 0x5a] {
            let state = RasterState {
                depth: DepthState { compare: DepthCompare::Always, write: false },
                stencil: StencilState {
                    enabled: true,
                    write_mask: 255,
                    reference,
                    pass: StencilOperation::Replace,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut batch = renderer.begin_batch(&target);
            batch.draw(&quad(), Viewport::full(&target), state).unwrap();
            let read = batch.submit_readback().unwrap();
            assert_eq!(read.read().unwrap().stencil, vec![reference; 64]);
            assert_eq!(renderer.raster_pipelines.pipelines.lock().unwrap().len(), 1, "references must stay dynamic");
        }
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn stencil_operations_comparisons_and_alpha_rejection_match_expected_pixels() {
        let renderer = Renderer::new().unwrap();
        for (operation, stored, expected) in
            [(0, 128, 128), (1, 128, 0), (2, 128, 0x91), (3, 255, 255), (4, 0, 0), (5, 0x5a, 0xa5), (6, 255, 0), (7, 0, 255)]
        {
            let target = renderer.create_color_target(8, 8).unwrap();
            let mut batch = renderer.begin_batch(&target);
            batch.seed_stencil(&[stored; 64], 1).unwrap();
            let state = RasterState {
                depth: DepthState { compare: DepthCompare::Always, write: false },
                stencil: StencilState::from_pica(0xff91ff11, operation << 8, 1, 3),
                ..Default::default()
            };
            batch.draw(&quad(), Viewport::full(&target), state).unwrap();
            let ticket = batch.submit_readback().unwrap();
            assert_eq!(ticket.read().unwrap().stencil, vec![expected; 64], "operation {operation}");
        }
        for (test, ops, depth_test) in [
            (0xff91ff01, 2, DepthCompare::Always),
            (0xff91ff11, 2 << 4, DepthCompare::Never),
            (0xff91ff11, 2 << 8, DepthCompare::Always),
            (0x0f910f21, 2 << 8, DepthCompare::Always),
        ] {
            for reject in [false, true] {
                let target = renderer.create_color_target(8, 8).unwrap();
                let mut batch = renderer.begin_batch(&target);
                batch.seed_stencil(&[0xa1; 64], 1).unwrap();
                let state = RasterState {
                    depth: DepthState { compare: depth_test, write: false },
                    stencil: StencilState::from_pica(test, ops, 1, 3),
                    alpha_test: reject.then_some(AlphaTest { compare: DepthCompare::Never, reference: 0 }),
                    ..Default::default()
                };
                batch.draw(&quad(), Viewport::full(&target), state).unwrap();
                let ticket = batch.submit_readback().unwrap();
                let expected = if reject || test == 0x0f910f21 { 0xa1 } else { 0x91 };
                assert_eq!(ticket.read().unwrap().stencil, vec![expected; 64]);
                let visible = !reject && test & 0xf0 != 0 && depth_test == DepthCompare::Always;
                assert!(ticket.read().unwrap().color.iter().all(|&b| b == if visible { 255 } else { 0 }));
            }
        }
    }
}

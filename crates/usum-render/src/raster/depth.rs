//! Preserve full-resolution depth across framebuffer alias changes.
use super::*;

impl Renderer {
    pub fn snapshot_depth(&self, source: &ColorTarget) -> Result<DepthSnapshot, String> {
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let extent = wgpu::Extent3d { width: source.width, height: source.height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frozen GPU depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("freeze GPU depth") });
        let image = |texture| wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::DepthOnly,
        };
        encoder.copy_texture_to_texture(image(&source.depth), image(&texture), extent);
        self.queue.submit([encoder.finish()]);
        check_errors(errors, "GPU depth snapshot")?;
        Ok(DepthSnapshot { texture, width: source.width, height: source.height })
    }
}

impl RasterBatch<'_> {
    pub fn restore_depth(&mut self, snapshot: &DepthSnapshot) -> Result<(), String> {
        if (snapshot.width, snapshot.height) != self.target.dimensions() {
            return Err("depth snapshot dimensions do not match target".into());
        }
        let encoder = self.encoder.as_mut().ok_or("Vulkan raster batch was already submitted")?;
        let image = |texture| wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::DepthOnly,
        };
        encoder.copy_texture_to_texture(
            image(&snapshot.texture),
            image(&self.target.depth),
            wgpu::Extent3d { width: snapshot.width, height: snapshot.height, depth_or_array_layers: 1 },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn depth_snapshot_survives_source_reuse_and_destruction() {
        let renderer = Renderer::new().unwrap();
        let source = renderer.create_color_target(8, 4).unwrap();
        let color = vec![0; 8 * 4 * 4];
        let expected: Vec<f32> = (0..32).map(|n| n as f32 / 31.0).collect();
        renderer.upload_target(&source, &color, &expected).unwrap();
        let snapshot = renderer.snapshot_depth(&source).unwrap();
        renderer.upload_target(&source, &color, &[1.0; 32]).unwrap();
        let mut batch = renderer.begin_batch(&source);
        batch.restore_depth(&snapshot).unwrap();
        assert_eq!(batch.finish_and_read_color_and_depth().unwrap().1, expected);
        drop(source);
        let destination = renderer.create_color_target(8, 4).unwrap();
        let mut batch = renderer.begin_batch(&destination);
        batch.restore_depth(&snapshot).unwrap();
        assert_eq!(batch.finish_and_read_color_and_depth().unwrap().1, expected);
    }
}

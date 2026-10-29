//! Keep scaled framebuffer texture reads resident across command lists.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plane {
    Color,
    DepthStencil,
}

#[derive(Clone, Debug)]
pub(super) struct ResidentTexture {
    plane: Plane,
    info: CaptureTarget,
    contents: Arc<TargetSnapshot>,
    pixels: Arc<[u8]>,
    sampler: SamplerState,
    border: [u8; 4],
    gpu: Arc<SampledTexture>,
}

pub(super) fn prune(pica: &mut Pica) {
    let targets = &pica.vulkan_live_target_cache;
    pica.vulkan_live_resources.resident_textures.retain(|entry| {
        targets.iter().any(|target| {
            target.info == entry.info && target.contents.as_ref().is_some_and(|contents| Arc::ptr_eq(contents, &entry.contents))
        })
    });
}

fn native_matches(contents: &TargetSnapshot, texture: &TextureSnapshot) -> bool {
    let (w, h) = (contents.width as usize, contents.height as usize);
    if w == 0
        || h == 0
        || texture.width != contents.width
        || texture.height != contents.height
        || texture.format != 0
        || texture.pixels.len() != w * h * 4
        || contents.depth.len() != w * h
        || contents.stencil.len() != w * h
    {
        return false;
    }
    for y in 0..h {
        for x in 0..w {
            let at = (h - 1 - y) * w + x;
            let depth = (contents.depth[at].clamp(0.0, 1.0) * 0xFF_FFFF as f32) as u32;
            let expected = depth.swap_bytes() | u32::from(contents.stencil[at]);
            let offset = (y * w + x) * 4;
            if u32::from_le_bytes(texture.pixels[offset..offset + 4].try_into().expect("RGBA texel")) != expected {
                return false;
            }
        }
    }
    true
}

pub(super) fn texture(
    pica: &mut Pica,
    renderer: &Renderer,
    texture: &TextureSnapshot,
) -> Result<Option<Arc<SampledTexture>>, String> {
    if pica.vulkan_resolution_scale == 1 || texture.format != 0 {
        return Ok(None);
    }
    let mut newest: Option<(usize, Plane)> = None;
    for (i, target) in pica.vulkan_live_target_cache.iter().enumerate() {
        let plane = if target.info.color_format == 0
            && compatible_color_texture(texture, target.info)
            && texture.address == pica.pa_to_va(target.info.color_address)?
        {
            Some(Plane::Color)
        } else if depth_texture_matches(pica, texture, target.info)? {
            Some(Plane::DepthStencil)
        } else {
            None
        };
        if let Some(plane) = plane {
            if newest.is_none_or(|(previous, _)| target.last_draw >= pica.vulkan_live_target_cache[previous].last_draw) {
                newest = Some((i, plane));
            }
        }
    }
    let Some((index, plane)) = newest else {
        return Ok(None);
    };
    let source = &pica.vulkan_live_target_cache[index];
    let Some(contents) = source.contents.as_ref().filter(|_| plane == Plane::Color || source.depth_detail) else {
        return Ok(None);
    };
    if std::env::var("USUM_VULKAN_RESIDENT_TEXTURES").as_deref() == Ok("0") {
        return Ok(None);
    }
    if plane == Plane::DepthStencil && std::env::var("USUM_VULKAN_RESIDENT_DEPTH_TEXTURE").as_deref() == Ok("0") {
        return Ok(None);
    }
    if let Some(entry) = pica.vulkan_live_resources.resident_textures.iter().find(|entry| {
        entry.info == source.info
            && entry.plane == plane
            && Arc::ptr_eq(&entry.contents, contents)
            && entry.sampler == texture.sampler
            && entry.border == texture.border
            && (Arc::ptr_eq(&entry.pixels, &texture.pixels) || entry.pixels == texture.pixels)
    }) {
        return Ok(Some(Arc::clone(&entry.gpu)));
    }
    if !(match plane {
        Plane::DepthStencil => native_matches(contents, texture),
        Plane::Color => color_matches(contents, texture),
    }) {
        return Ok(None);
    }
    let gpu = Arc::new(match plane {
        Plane::DepthStencil => renderer.create_d24s8_feedback_texture(&source.gpu, texture.sampler, texture.border)?,
        Plane::Color => renderer.create_color_feedback_texture(&source.gpu, texture.sampler, texture.border)?,
    });
    pica.vulkan_live_resources.resident_textures.retain(|entry| {
        entry.info != source.info || entry.plane != plane || entry.sampler != texture.sampler || entry.border != texture.border
    });
    pica.vulkan_live_resources.resident_textures.push(ResidentTexture {
        plane,
        info: source.info,
        contents: Arc::clone(contents),
        pixels: Arc::clone(&texture.pixels),
        sampler: texture.sampler,
        border: texture.border,
        gpu: Arc::clone(&gpu),
    });
    Ok(Some(gpu))
}

fn color_matches(contents: &TargetSnapshot, texture: &TextureSnapshot) -> bool {
    let row = contents.width as usize * 4;
    row != 0
        && contents.height != 0
        && texture.format == 0
        && texture.width == contents.width
        && texture.height == contents.height
        && contents.color.len() == row * contents.height as usize
        && texture.pixels.len() == contents.color.len()
        && contents.color.chunks_exact(row).rev().zip(texture.pixels.chunks_exact(row)).all(|(a, b)| a == b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn depth_texture_proof_checks_row_order_depth_bytes_and_stencil() {
        let mut contents = TargetSnapshot {
            width: 2,
            height: 2,
            color: vec![0; 16],
            depth: vec![0.0, 1.0, 0.5, 0.25],
            stencil: vec![0x12, 0x34, 0x56, 0x78],
        };
        let mut texture = TextureSnapshot {
            address: 0,
            source_size: 16,
            format: 0,
            width: 2,
            height: 2,
            pixels: Arc::from([0x56, 0x7f, 0xff, 0xff, 0x78, 0x3f, 0xff, 0xff, 0x12, 0, 0, 0, 0x34, 0xff, 0xff, 0xff]),
            sampler: Default::default(),
            border: [0; 4],
        };
        assert!(native_matches(&contents, &texture));
        contents.stencil[0] = 0x13;
        assert!(!native_matches(&contents, &texture));
        contents.stencil[0] = 0x12;
        contents.depth[1] = 0.75;
        assert!(!native_matches(&contents, &texture));
        contents.depth[1] = 1.0;
        texture.width = 1;
        assert!(!native_matches(&contents, &texture));
        texture.width = 2;
        texture.format = 1;
        assert!(!native_matches(&contents, &texture));
    }

    #[test]
    fn color_texture_proof_checks_rows_and_all_channels() {
        let mut contents = TargetSnapshot { width: 2, height: 2, color: (0..16).collect(), depth: vec![], stencil: vec![] };
        let texture = TextureSnapshot {
            address: 0,
            source_size: 16,
            format: 0,
            width: 2,
            height: 2,
            pixels: Arc::from([8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7]),
            sampler: Default::default(),
            border: [0; 4],
        };
        assert!(color_matches(&contents, &texture));
        contents.color[3] ^= 1;
        assert!(!color_matches(&contents, &texture));
    }

    fn pixels(pica: &Pica, mem: &Memory, info: CaptureTarget, plane: Plane) -> Arc<[u8]> {
        let current = target_snapshot(pica, mem, info).unwrap();
        match plane {
            Plane::DepthStencil => d24s8_rgba8_texture(pica, mem, info, &current.depth).unwrap(),
            Plane::Color => {
                current.color.chunks_exact(info.width as usize * 4).rev().flatten().copied().collect::<Vec<_>>().into()
            }
        }
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn later_lists_sample_full_resolution_planes_and_refresh_identical_native_results() {
        use usum_render::{ColorMask, DepthCompare, DepthState, StencilState, TevStage, TevState, Viewport};
        let renderer = live_renderer().unwrap();
        for (plane, defer) in
            [(Plane::DepthStencil, false), (Plane::Color, false), (Plane::DepthStencil, true), (Plane::Color, true)]
        {
            let mut pica = Pica::default();
            pica.enable_vulkan_acceleration();
            if defer {
                pica.enable_vulkan_deferred_readback();
            }
            pica.set_vulkan_resolution_scale(6).unwrap();
            pica.regs[0x41] = 0x430000;
            pica.regs[0x43] = 0x430000;
            pica.regs[0x4d] = 0xbf0000;
            pica.regs[0x6d] = 1;
            let info = CaptureTarget {
                color_address: 0x20000000,
                depth_address: 0x20001000,
                width: 32,
                height: 32,
                color_format: 0,
                depth_format: 3,
            };
            let output = CaptureTarget { color_address: 0x20002000, depth_address: 0, ..info };
            let mut mem = Memory::new();
            mem.map(pica.linear_base, vec![0; 0x4000], usum_guest::Perm::RW).unwrap();
            let triangles = [[0, 1, 2], [0, 2, 3]];
            let vertex = |x, y, u, v| Vertex {
                position: [x, y, -0.5, 1.0],
                color: [1.0; 4],
                texcoord: [[u, v, 1.0]; 3],
                ..Vertex::default()
            };
            let screen = [
                vertex(-1.0, -1.0, 0.0, 0.0),
                vertex(1.0, -1.0, 1.0, 0.0),
                vertex(1.0, 1.0, 1.0, 1.0),
                vertex(-1.0, 1.0, 0.0, 1.0),
            ];
            let mut native_pixels: Option<Arc<[u8]>> = None;
            let mut prior_high_resolution: Option<Vec<u8>> = None;
            for (phase, right) in [0.015625, 0.02734375].into_iter().enumerate() {
                mem.device_write(pica.linear_base + 0x1000, &[255; 4096]).unwrap();
                let shape = [
                    vertex(-1.0, -1.0, 0.0, 0.0),
                    vertex(right, -1.0, 0.0, 0.0),
                    vertex(right, 1.0, 0.0, 0.0),
                    vertex(-1.0, 1.0, 0.0, 0.0),
                ];
                let (target, textures) = capture_buffers(&mut pica, &mem, info).unwrap();
                let mut draw =
                    DrawSnapshot::from_shaded_vertices_shared(&pica, &shape, &triangles, info, target, textures).unwrap();
                draw.draw_number = (phase * 2 + 1) as u64;
                draw.state.lighting = None;
                draw.state.raster.depth = DepthState { compare: DepthCompare::Always, write: false };
                draw.state.raster.color_mask = if plane == Plane::Color { ColorMask::ALL } else { ColorMask::from_bits(0) };
                draw.state.raster.tev = TevState::default();
                draw.state.raster.stencil = if plane == Plane::DepthStencil {
                    StencilState::from_pica(0xff91ff11, 0x200, 1, 3)
                } else {
                    StencilState::default()
                };
                pica.vulkan_snapshots.push(draw);
                replay_live(&mut pica, &mut mem).unwrap();
                let source = pica.vulkan_live_target_cache.iter().find(|t| t.info == info).unwrap();
                assert_eq!(source.pending.is_some(), defer);
                let materialized = pica.vulkan_live_resources.deferred.get().1;
                let pixels = pixels(&pica, &mem, info, plane);
                if defer {
                    assert_eq!(pica.vulkan_live_resources.deferred.get().1, materialized + 1);
                }
                if let Some(prior) = &native_pixels {
                    assert_eq!(&**prior, &*pixels, "native samples deliberately stay identical");
                }
                let pixels = Arc::clone(native_pixels.get_or_insert(pixels));
                let texture = TextureSnapshot {
                    address: pica.linear_base + if plane == Plane::Color { 0 } else { 0x1000 },
                    source_size: 4096,
                    format: 0,
                    width: 32,
                    height: 32,
                    pixels,
                    sampler: Default::default(),
                    border: [0; 4],
                };
                let expected_texture = match plane {
                    Plane::DepthStencil => {
                        renderer.create_d24s8_feedback_texture(&source.gpu, texture.sampler, texture.border).unwrap()
                    }
                    Plane::Color => renderer.create_color_feedback_texture(&source.gpu, texture.sampler, texture.border).unwrap(),
                };
                let (target, mut textures) = capture_buffers(&mut pica, &mem, output).unwrap();
                textures[0] = Some(texture);
                let mut draw =
                    DrawSnapshot::from_shaded_vertices_shared(&pica, &screen, &triangles, output, target, textures).unwrap();
                draw.draw_number = (phase * 2 + 2) as u64;
                draw.state.lighting = None;
                draw.state.raster.depth = DepthState { compare: DepthCompare::Always, write: false };
                draw.state.raster.tev = TevState::default();
                draw.state.raster.tev.stages[0] = TevStage::replace(3);
                draw.state.raster.color_mask = ColorMask::ALL;
                let output_state = draw.state.raster;
                let output_vertices = draw.vertices.clone();
                pica.vulkan_snapshots.push(draw);
                replay_live(&mut pica, &mut mem).unwrap();
                let actual = renderer
                    .read_color_target(&pica.vulkan_live_target_cache.iter().find(|t| t.info == output).unwrap().gpu)
                    .unwrap();
                let reference = renderer.create_color_target(192, 192).unwrap();
                renderer
                    .draw_vertices_with_inputs(
                        &reference,
                        &output_vertices,
                        Viewport::full(&reference),
                        output_state,
                        FragmentInputs { texture0: Some(&expected_texture), ..Default::default() },
                    )
                    .unwrap();
                let expected = renderer.read_color_target(&reference).unwrap();
                assert_eq!(actual.len(), expected.len());
                assert_eq!(
                    actual.iter().zip(&expected).filter(|(a, b)| a != b).count(),
                    0,
                    "full-resolution sample at phase {phase}"
                );
                if let Some(prior) = prior_high_resolution {
                    assert_ne!(actual, prior, "subpixel geometry changes despite identical native bytes");
                }
                prior_high_resolution = Some(actual);
                assert_eq!(pica.vulkan_live_resources.resident_textures.len(), 1);
            }
            mem.write_u8(pica.linear_base + if plane == Plane::Color { 3 } else { 0x1003 }, 0x33).unwrap();
            let pixels = pixels(&pica, &mem, info, plane);
            let texture = TextureSnapshot {
                address: pica.linear_base + if plane == Plane::Color { 0 } else { 0x1000 },
                source_size: 4096,
                format: 0,
                width: 32,
                height: 32,
                pixels,
                sampler: Default::default(),
                border: [0; 4],
            };
            assert!(super::texture(&mut pica, renderer, &texture).unwrap().is_none(), "CPU writes reject resident reuse");
        }
    }
}

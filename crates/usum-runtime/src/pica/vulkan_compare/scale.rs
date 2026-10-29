//! Integer internal rendering scale and full-resolution LCD output.
use super::*;

fn trace(message: impl FnOnce() -> String) {
    if std::env::var_os("USUM_SCALE_TRACE").is_none() {
        return;
    }
    thread_local! {static SEEN:std::cell::RefCell<std::collections::HashSet<String>>=std::cell::RefCell::new(std::collections::HashSet::new());}
    SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        if seen.len() < 64 {
            let message = message();
            if seen.insert(message.clone()) {
                eprintln!("scale trace: {message}");
            }
        }
    });
}

#[derive(Clone, Debug)]
pub(in crate::pica) struct Display {
    address: u32,
    format: u32,
    expected: Arc<[u8]>,
    frame: crate::probe::ScaledFrame,
}

pub(super) fn expand<T: Copy>(input: &[T], width: u32, height: u32, scale: u32, channels: usize) -> Vec<T> {
    let mut out = Vec::with_capacity(input.len() * scale as usize * scale as usize);
    for y in 0..height {
        for _ in 0..scale {
            for x in 0..width {
                let at = (y * width + x) as usize * channels;
                for _ in 0..scale {
                    out.extend_from_slice(&input[at..at + channels]);
                }
            }
        }
    }
    out
}

pub(super) fn seed(input: &TargetSnapshot, scale: u32) -> TargetSnapshot {
    TargetSnapshot {
        width: input.width * scale,
        height: input.height * scale,
        color: expand(&input.color, input.width, input.height, scale, 4),
        depth: expand(&input.depth, input.width, input.height, scale, 1),
    }
}

pub(super) fn resolve(mut result: LiveResult, scale: u32) -> LiveResult {
    if scale == 1 {
        return result;
    }
    let info = result.info;
    let width = info.width * scale;
    let mut native_color = Vec::with_capacity((info.width * info.height * 4) as usize);
    let mut native_depth = Vec::with_capacity((info.width * info.height) as usize);
    for y in 0..info.height {
        for x in 0..info.width {
            let at = ((y * scale + scale / 2) * width + x * scale + scale / 2) as usize;
            native_color.extend_from_slice(&result.color[at * 4..at * 4 + 4]);
            if let Some(depth) = &result.depth {
                native_depth.push(depth[at]);
            }
        }
    }
    result.scaled_color = Some(std::mem::replace(&mut result.color, native_color).into());
    if result.depth.is_some() {
        result.depth = Some(native_depth);
    }
    result
}

pub(in crate::pica) fn capture_transfer(
    pica: &mut Pica,
    input_address: u32,
    output_address: u32,
    dims: ((u32, u32), (u32, u32)),
    flags: u32,
    input: &[u8],
    output: &[u8],
) -> Result<(), String> {
    pica.scaled_displays.retain(|d| d.address != output_address);
    let scale = pica.vulkan_resolution_scale;
    trace(|| format!("transfer {input_address:#x} {dims:?} -> {output_address:#x}, flags {flags:#x}, scale {scale}"));
    if scale == 1 || flags & 0x1_002a != 0 || (flags >> 8) & 7 != 0 || !matches!((flags >> 12) & 7, 0 | 1) {
        return Ok(());
    }
    let (source_dims, output_dims) = dims;
    let (sx, sy) = match flags >> 24 & 3 {
        0 => (1, 1),
        1 => (2, 1),
        2 => (2, 2),
        _ => return Ok(()),
    };
    let (ow, oh) = (output_dims.0.min(source_dims.0 / sx), output_dims.1.min(source_dims.1 / sy));
    // LCD memory is stored rotated: each row is a screen column.
    if ow != 240 || !matches!(oh, 320 | 400) {
        return Ok(());
    }
    let mut found = None;
    for (index, target) in pica.vulkan_live_target_cache.iter().enumerate() {
        let address = pica.pa_to_va(target.info.color_address)?;
        if let Some(origin) = transfer_origin(target.info, address, input_address, source_dims, (ow * sx, oh * sy)) {
            found = Some((index, origin));
            break;
        }
    }
    let Some((index, origin)) = found else {
        trace(|| {
            format!(
                "no source target for {input_address:#x} {source_dims:?}; targets {:?}",
                pica.vulkan_live_target_cache
                    .iter()
                    .map(|t| (pica.pa_to_va(t.info.color_address), t.info.width, t.info.height, t.info.color_format))
                    .collect::<Vec<_>>()
            )
        });
        return Ok(());
    };
    let target = &pica.vulkan_live_target_cache[index];
    let Some(contents) = &target.contents else {
        trace(|| format!("source {input_address:#x}: missing native contents"));
        return Ok(());
    };
    // CPU writes or an aliased target must not leave a stale high-resolution image.
    for y in 0..oh * sy {
        for x in 0..ow * sx {
            let at = tiled_offset(x, y, source_dims.0) as usize * 4;
            let src = ((origin.1 + y) * target.info.width + origin.0 + x) as usize * 4;
            let pixel = &contents.color[src..src + 4];
            if input.get(at..at + 4) != Some(&[pixel[3], pixel[2], pixel[1], pixel[0]]) {
                trace(|| format!("source {input_address:#x}: native bytes differ at {x},{y}"));
                return Ok(());
            }
        }
    }
    // Native guest coherence needs only center samples. Read the full-resolution
    // color plane once, when the game actually transfers this target to an LCD.
    trace(|| format!("scaled LCD hit {input_address:#x} -> {output_address:#x} {ow}x{oh}"));
    let rgb = if std::env::var("USUM_VULKAN_GPU_DISPLAY_COPY").as_deref() != Ok("0") {
        live_renderer()?.read_display_rgb(
            &target.gpu,
            usum_render::DisplayCopy { origin, size: (ow, oh), scale, downscale: (sx, sy), flip: flags & 1 != 0 },
        )?
    } else {
        let color = match &target.scaled_color {
            Some(color) => Arc::clone(color),
            None => live_renderer()?.read_color_target(&target.gpu)?.into(),
        };
        let rgb = lcd_rgb(&color, (target.info.width, target.info.height), (ow, oh), scale, (sx, sy), flags & 1 != 0, origin);
        pica.vulkan_live_target_cache[index].scaled_color = Some(color);
        rgb
    };
    let (width, height) = (oh * scale, ow * scale);
    if pica.scaled_displays.len() > 16 {
        pica.scaled_displays.clear();
    }
    pica.scaled_displays.push(Display {
        address: output_address,
        format: (flags >> 12) & 7,
        expected: output.to_vec().into(),
        frame: crate::probe::ScaledFrame { width, height, rgb: rgb.into() },
    });
    Ok(())
}

/// A transfer may start at a tile-aligned subregion of a padded target. The
/// advertised input height can extend beyond it; only cropped output is read.
fn transfer_origin(info: CaptureTarget, base: u32, input: u32, source: (u32, u32), used: (u32, u32)) -> Option<(u32, u32)> {
    if info.color_format != 0 || source.0 != info.width || info.width == 0 || !info.width.is_multiple_of(8) {
        return None;
    }
    let offset = input.checked_sub(base)?;
    if !offset.is_multiple_of(64 * 4) {
        return None;
    }
    let tile = offset / (64 * 4);
    let origin = (tile % (info.width / 8) * 8, tile / (info.width / 8) * 8);
    (origin.0.checked_add(used.0)? <= info.width && origin.1.checked_add(used.1)? <= info.height).then_some(origin)
}

fn lcd_rgb(
    color: &[u8],
    source_dims: (u32, u32),
    (ow, oh): (u32, u32),
    scale: u32,
    (sx, sy): (u32, u32),
    flip: bool,
    origin: (u32, u32),
) -> Vec<u8> {
    let (width, height) = (oh * scale, ow * scale);
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    let sw = source_dims.0 * scale;
    if sx == 1 && sy == 1 {
        for y in 0..height {
            let source_x = origin.0 * scale + height - 1 - y;
            for x in 0..width {
                let source_y = origin.1 * scale + if flip { width - 1 - x } else { x };
                let at = (source_y * sw + source_x) as usize * 4;
                rgb.extend_from_slice(&color[at..at + 3]);
            }
        }
        return rgb;
    }
    for y in 0..height {
        for x in 0..width {
            let output_row = if flip { width - 1 - x } else { x };
            let source_x = origin.0 * scale + (height - 1 - y) * sx;
            let source_y = origin.1 * scale + output_row * sy;
            let mut sum = [0u32; 3];
            for dy in 0..sy {
                for dx in 0..sx {
                    let at = ((source_y + dy) * sw + source_x + dx) as usize * 4;
                    for c in 0..3 {
                        sum[c] += u32::from(color[at + c]);
                    }
                }
            }
            rgb.extend(sum.map(|v| ((v + sx * sy / 2) / (sx * sy)) as u8));
        }
    }
    rgb
}

pub(in crate::pica) fn presented(
    pica: &Pica,
    p: &crate::kernel::Present,
    columns: u32,
    raw: &crate::probe::RawFrame,
) -> Option<crate::probe::ScaledFrame> {
    trace(|| {
        format!(
            "present {:#x} format {} stride {} columns {} cached {}",
            p.left,
            p.format,
            p.stride,
            columns,
            pica.scaled_displays.len()
        )
    });
    pica.scaled_displays
        .iter()
        .find(|d| {
            d.address == p.left
                && d.format == (p.format & 7)
                && d.frame.width == columns * pica.vulkan_resolution_scale
                && bytes_per_pixel(d.format).is_some_and(|bpp| p.stride == 240 * bpp as u32)
                && d.expected.as_ref() == raw.bytes
        })
        .map(|d| d.frame.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn gpu_display_matches_cpu_crops_flips_downscales_and_rgb_tail_packing() {
        let renderer = Renderer::new().unwrap();
        for scale in [1, 2, 3, 6, 8] {
            let (width, height) = (72, 64);
            let color: Vec<u8> = (0..width * height * scale * scale * 4).map(|n| (n * 13 + n / 257) as u8).collect();
            let depth = vec![1.0; (width * height * scale * scale) as usize];
            let target = renderer.create_color_target_from_snapshot(width * scale, height * scale, &color, &depth).unwrap();
            for downscale in [(1, 1), (2, 1), (1, 2), (2, 2)] {
                for flip in [false, true] {
                    let copy = usum_render::DisplayCopy { origin: (7, 5), size: (19, 23), scale, downscale, flip };
                    let actual = renderer.read_display_rgb(&target, copy).unwrap();
                    let expected = lcd_rgb(&color, (width, height), copy.size, scale, downscale, flip, copy.origin);
                    assert_eq!(actual, expected, "scale {scale}, downscale {downscale:?}, flip {flip}");
                }
            }
        }
    }

    #[test]
    fn lcd_views_start_inside_padded_targets_and_read_only_the_crop() {
        let info =
            CaptureTarget { color_address: 0, depth_address: 0, width: 256, height: 512, color_format: 0, depth_format: 3 };
        assert_eq!(transfer_origin(info, 0x1f00_0000, 0x1f01_c000, (256, 512), (240, 400)), Some((0, 112)));
        assert_eq!(transfer_origin(info, 0x1f00_0000, 0x1f03_0000, (256, 512), (240, 320)), Some((0, 192)));
        assert_eq!(transfer_origin(info, 0x1f00_0000, 0x1f01_c100, (256, 512), (240, 400)), Some((8, 112)));
        assert_eq!(transfer_origin(info, 0x1f00_0000, 0x1f01_c000, (256, 512), (240, 401)), None);
        assert_eq!(transfer_origin(info, 0x1f00_0000, 0x1f01_c004, (256, 512), (240, 400)), None);
        assert_eq!(transfer_origin(info, 0x1f00_0000, 0x1f01_c000, (128, 512), (120, 400)), None);
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn six_x_subregion_survives_transfer_rotation_and_presentation() {
        let renderer = live_renderer().unwrap();
        let mut pica = Pica::default();
        pica.enable_vulkan_acceleration();
        pica.set_vulkan_resolution_scale(6).unwrap();
        let info = CaptureTarget {
            color_address: 0x2000_0000,
            depth_address: 0,
            width: 256,
            height: 512,
            color_format: 0,
            depth_format: 3,
        };
        let gpu = Arc::new(renderer.create_color_target(1536, 3072).unwrap());
        renderer
            .draw_vertices(
                &gpu,
                &[
                    RasterVertex::new([-0.751, -0.753, -0.5, 1.0], [1.0; 4]),
                    RasterVertex::new([0.749, -0.751, -0.5, 1.0], [1.0; 4]),
                    RasterVertex::new([0.003, 0.747, -0.5, 1.0], [1.0; 4]),
                ],
            )
            .unwrap();
        let (color, depth) = renderer.read_color_and_depth_target(&gpu).unwrap();
        let result = resolve(LiveResult { info, color, depth: Some(depth), pending: None, scaled_color: None }, 6);
        let native = read_native_result(renderer.begin_batch(&gpu), info, 6).unwrap();
        assert_eq!(native.color, result.color);
        assert_eq!(native.depth, result.depth);
        let expected_lcd = lcd_rgb(result.scaled_color.as_ref().unwrap(), (256, 512), (240, 400), 6, (1, 1), false, (0, 112));
        let mut tiled = vec![0; 256 * 512 * 4];
        encode_rgba8_target(&result.color, &mut tiled, 256);
        pica.vulkan_live_target_cache.push(LiveTarget {
            last_draw: 0,
            depth_detail: true,
            info,
            gpu,
            contents: Some(TargetSnapshot { width: 256, height: 512, color: result.color, depth: result.depth.unwrap() }),
            pending: None,
            scaled_color: None,
        });
        let mut memory = Memory::new();
        memory.map(pica.linear_base, vec![0; 0x20_0000], usum_guest::Perm::RW).unwrap();
        memory.device_write(pica.linear_base, &tiled).unwrap();
        let input = memory.read_range(pica.linear_base + 0x1c000, 256 * 512 * 4).unwrap();
        let output = super::super::super::display_transfer(&input, (256, 512), (240, 400), 0x1004).unwrap();
        let address = pica.linear_base + 0x10_0000;
        capture_transfer(
            &mut pica,
            memory.regions().next().unwrap().0 + 0x1c000,
            address,
            ((256, 512), (240, 400)),
            0x1004,
            &input,
            &output,
        )
        .unwrap();
        let present = crate::kernel::Present { screen: 0, active: 0, left: address, right: 0, stride: 720, format: 0x141 };
        let mut raw = crate::probe::RawFrame { bytes: output, format: 1, stride: 720, scaled: None };
        let frame = presented(&pica, &present, 400, &raw).expect("cropped high-resolution frame reaches the window");
        assert_eq!((frame.width, frame.height), (2400, 1440));
        assert_eq!(frame.rgb.as_ref(), expected_lcd);
        assert!(
            pica.vulkan_live_target_cache[0].scaled_color.is_none(),
            "LCD transfer keeps the full-resolution RGBA plane on the GPU"
        );
        let varied = (0..240).any(|y| {
            (0..400).any(|x| {
                let at = (y * 6 * 2400 + x * 6) * 3;
                (0..6)
                    .any(|dy| (0..6).any(|dx| frame.rgb[((y * 6 + dy) * 2400 + x * 6 + dx) * 3..][..3] != frame.rgb[at..at + 3]))
            })
        });
        assert!(varied, "presented native pixel blocks must retain subpixel raster edges");
        raw.bytes[0] ^= 1;
        assert!(presented(&pica, &present, 400, &raw).is_none(), "CPU changes invalidate stale display snapshots");
        let mut changed_input = input;
        changed_input[0] ^= 1;
        capture_transfer(
            &mut pica,
            memory.regions().next().unwrap().0 + 0x1c000,
            address,
            ((256, 512), (240, 400)),
            0x1004,
            &changed_input,
            &raw.bytes,
        )
        .unwrap();
        assert!(pica.scaled_displays.is_empty(), "CPU framebuffer writes invalidate the scaled source before readback");
    }

    #[test]
    fn expanded_seed_and_guest_resolve_keep_native_memory_dimensions() {
        let original = TargetSnapshot { width: 2, height: 1, color: vec![1, 2, 3, 4, 5, 6, 7, 8], depth: vec![0.25, 0.75] };
        let expanded = seed(&original, 6);
        assert_eq!((expanded.width, expanded.height), (12, 6));
        let result = resolve(
            LiveResult {
                info: CaptureTarget { color_address: 0, depth_address: 1, width: 2, height: 1, color_format: 0, depth_format: 3 },
                color: expanded.color,
                depth: Some(expanded.depth),
                pending: None,
                scaled_color: None,
            },
            6,
        );
        assert_eq!(result.color, original.color);
        assert_eq!(result.depth.unwrap(), original.depth);
        assert_eq!(result.scaled_color.unwrap().len(), 12 * 6 * 4);
    }
    #[test]
    fn lcd_rotation_flip_crop_and_downscale_match_native_transfer() {
        for (source_dims, output_dims, sampling) in
            [((256, 512), (240, 400), 0), ((512, 512), (240, 400), 1), ((512, 1024), (240, 400), 2)]
        {
            let (sw, sh) = source_dims;
            let mut color = vec![0; (sw * sh * 4) as usize];
            let mut tiled = vec![0; color.len()];
            for y in 0..sh {
                for x in 0..sw {
                    let at = ((y * sw + x) * 4) as usize;
                    color[at..at + 4].copy_from_slice(&[(x * 7) as u8, (y * 13) as u8, (x + y) as u8, 255]);
                }
            }
            encode_rgba8_target(&color, &mut tiled, sw);
            for flip in [false, true] {
                let flags = 1 << 12 | 4 | sampling << 24 | u32::from(flip);
                let bytes = super::super::super::display_transfer(&tiled, source_dims, output_dims, flags).unwrap();
                let raw = crate::probe::RawFrame { bytes, format: 1, stride: 720, scaled: None };
                let expected = crate::probe::raw_to_rgb(&raw, 400);
                let axes = match sampling {
                    0 => (1, 1),
                    1 => (2, 1),
                    _ => (2, 2),
                };
                assert_eq!(lcd_rgb(&color, source_dims, output_dims, 1, axes, flip, (0, 0)), expected);
                if sampling == 0 {
                    let high = expand(&color, sw, sh, 2, 4);
                    assert_eq!(
                        lcd_rgb(&high, source_dims, output_dims, 2, axes, flip, (0, 0)),
                        expand(&expected, 400, 240, 2, 3)
                    );
                }
            }
        }
    }
    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn six_x_replay_rasterizes_subpixels_and_preserves_native_guest_buffers() {
        let mut pica = Pica::default();
        pica.enable_vulkan_acceleration();
        pica.set_vulkan_resolution_scale(6).unwrap();
        pica.regs[0x41] = 0x430000;
        pica.regs[0x43] = 0x430000;
        pica.regs[0x107] = 1 | 5 << 4 | 0xF << 8 | 1 << 12;
        pica.regs[0x113] = 1;
        pica.regs[0x115] = 2;
        let info = CaptureTarget {
            color_address: 0x2000_0000,
            depth_address: 0x2000_1000,
            width: 32,
            height: 32,
            color_format: 0,
            depth_format: 3,
        };
        let mut mem = Memory::new();
        mem.map(pica.linear_base, vec![0; 0x2000], usum_guest::Perm::RW).unwrap();
        mem.device_write(pica.linear_base + 0x1000, &[255; 4096]).unwrap();
        let vertices = [
            Vertex { position: [-0.751, -0.753, -0.5, 1.0], color: [1.0; 4], ..Vertex::default() },
            Vertex { position: [0.749, -0.751, -0.5, 1.0], color: [1.0; 4], ..Vertex::default() },
            Vertex { position: [0.003, 0.747, -0.5, 1.0], color: [1.0; 4], ..Vertex::default() },
        ];
        let (target, textures) = capture_buffers(&mut pica, &mem, info).unwrap();
        pica.vulkan_snapshots
            .push(DrawSnapshot::from_shaded_vertices_shared(&pica, &vertices, &[[0, 1, 2]], info, target, textures).unwrap());
        replay_live(&mut pica, &mut mem).unwrap();
        let target = &pica.vulkan_live_target_cache[0];
        assert_eq!(target.gpu.dimensions(), (192, 192));
        assert!(target.scaled_color.is_none(), "command lists keep high-resolution color on the GPU");
        let color = live_renderer().unwrap().read_color_target(&target.gpu).unwrap();
        assert_eq!(color.len(), 192 * 192 * 4);
        let has_subpixel_edges = (0..32).any(|y| {
            (0..32).any(|x| {
                let at = (y * 6 * 192 + x * 6) * 4;
                (0..6).any(|dy| (0..6).any(|dx| color[(y * 6 + dy) * 192 * 4 + (x * 6 + dx) * 4..][..4] != color[at..at + 4]))
            })
        });
        assert!(has_subpixel_edges, "actual raster edges must vary inside native pixel blocks");
        assert_eq!(mem.read_range(pica.linear_base, 4096).unwrap().len(), 4096);
        assert_eq!(mem.read_range(pica.linear_base + 0x1000, 4096).unwrap().len(), 4096);
    }
}

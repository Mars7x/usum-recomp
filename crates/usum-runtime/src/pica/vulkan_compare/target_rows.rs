//! Convert complete Morton tile rows with one base calculation per eight pixels.
//! Ineligible shapes retain the scalar converters and their bounds behavior.
use super::tiled_offset;

const X: [usize; 8] = [0, 1, 4, 5, 16, 17, 20, 21];

pub(super) fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("USUM_VULKAN_TARGET_TILE_ROWS").as_deref() != Ok("0"))
}

fn eligible(width: u32, pixels: usize, tiled_len: usize) -> bool {
    width != 0
        && width.is_multiple_of(8)
        && (width as usize).checked_mul(8).is_some_and(|tile_row| pixels.is_multiple_of(tile_row))
        && pixels.checked_mul(4).is_some_and(|bytes| bytes <= tiled_len)
}

pub(super) fn decode_color(raw: &[u8], output: &mut [u8], width: u32) -> bool {
    if !output.len().is_multiple_of(4) || !eligible(width, output.len() / 4, raw.len()) {
        return false;
    }
    for (row, destination) in output.chunks_exact_mut(width as usize * 4).enumerate() {
        let base = tiled_offset(0, row as u32, width) as usize * 4;
        for (tile, pixels) in destination.as_chunks_mut::<32>().0.iter_mut().enumerate() {
            let source = &raw[base + tile * 256..base + tile * 256 + 88];
            for (pixel, x) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(X) {
                let at = x * 4;
                pixel.copy_from_slice(&[source[at + 3], source[at + 2], source[at + 1], source[at]]);
            }
        }
    }
    true
}

pub(super) fn encode_color(color: &[u8], tiled: &mut [u8], width: u32) -> bool {
    if !color.len().is_multiple_of(4) || !eligible(width, color.len() / 4, tiled.len()) {
        return false;
    }
    for (row, source) in color.chunks_exact(width as usize * 4).enumerate() {
        let base = tiled_offset(0, row as u32, width) as usize * 4;
        for (tile, pixels) in source.as_chunks::<32>().0.iter().enumerate() {
            let destination = &mut tiled[base + tile * 256..base + tile * 256 + 88];
            for (&[r, g, b, a], x) in pixels.as_chunks::<4>().0.iter().zip(X) {
                destination[x * 4..x * 4 + 4].copy_from_slice(&[a, b, g, r]);
            }
        }
    }
    true
}

pub(super) fn decode_depth(raw: &[u8], output: &mut [f32], stencil: &mut [u8], width: u32) -> bool {
    if stencil.len() != output.len() || !eligible(width, output.len(), raw.len()) {
        return false;
    }
    for (row, destination) in output.chunks_exact_mut(width as usize).enumerate() {
        let base = tiled_offset(0, row as u32, width) as usize * 4;
        let stencil_row = &mut stencil[row * width as usize..(row + 1) * width as usize];
        for (tile, pixels) in destination.as_chunks_mut::<8>().0.iter_mut().enumerate() {
            let source = &raw[base + tile * 256..base + tile * 256 + 88];
            for ((pixel, stencil), x) in pixels.iter_mut().zip(&mut stencil_row[tile * 8..tile * 8 + 8]).zip(X) {
                let value = u32::from_le_bytes(source[x * 4..x * 4 + 4].try_into().expect("D24S8 pixel"));
                *pixel = (value & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32;
                *stencil = (value >> 24) as u8;
            }
        }
    }
    true
}

pub(super) fn encode_depth(depth: &[f32], stencil: Option<&[u8]>, tiled: &mut [u8], width: u32) -> bool {
    if stencil.is_some_and(|s| s.len() != depth.len()) || !eligible(width, depth.len(), tiled.len()) {
        return false;
    }
    for (row, source) in depth.chunks_exact(width as usize).enumerate() {
        let base = tiled_offset(0, row as u32, width) as usize * 4;
        for (tile, pixels) in source.as_chunks::<8>().0.iter().enumerate() {
            let destination = &mut tiled[base + tile * 256..base + tile * 256 + 88];
            for ((column, value), x) in pixels.iter().enumerate().zip(X) {
                let value = (value.clamp(0.0, 1.0) * 0xFF_FFFF as f32) as u32;
                let at = x * 4;
                let stencil = stencil.map_or(destination[at + 3], |s| s[row * width as usize + tile * 8 + column]);
                destination[at..at + 4].copy_from_slice(&(value | (u32::from(stencil) << 24)).to_le_bytes());
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed
    }

    #[test]
    fn complete_color_and_depth_tile_rows_match_scalar_morton_oracles_bit_for_bit() {
        let mut seed = 0x853cad17;
        for (width, height) in [(8, 8), (16, 8), (24, 16), (240, 400), (256, 512), (512, 256)] {
            let pixels = width as usize * height as usize;
            let raw: Vec<u8> = (0..pixels * 4).map(|_| (next(&mut seed) >> 24) as u8).collect();
            let mut color = vec![0; pixels * 4];
            let mut depth = vec![0.0; pixels];
            let mut stencil = vec![0; pixels];
            assert!(decode_color(&raw, &mut color, width));
            assert!(decode_depth(&raw, &mut depth, &mut stencil, width));
            for y in 0..height {
                for x in 0..width {
                    let linear = (y * width + x) as usize;
                    let at = tiled_offset(x, y, width) as usize * 4;
                    assert_eq!(&color[linear * 4..linear * 4 + 4], &[raw[at + 3], raw[at + 2], raw[at + 1], raw[at]]);
                    let value = u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
                    assert_eq!(depth[linear].to_bits(), ((value & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32).to_bits());
                    assert_eq!(stencil[linear], (value >> 24) as u8);
                }
            }
            let mut color_tiled = vec![0xa5; raw.len() + 17];
            assert!(encode_color(&color, &mut color_tiled, width));
            assert_eq!(&color_tiled[..raw.len()], &raw);
            assert!(color_tiled[raw.len()..].iter().all(|&b| b == 0xa5));

            // Includes arbitrary NaNs, subnormals, infinities, negative and
            // out-of-range values, as well as exact endpoints/rounding neighbors.
            let boundaries = [0, 0x8000_0000, 1, 0x007f_ffff, 0x3f7f_ffff, 0x3f80_0000, 0x7f80_0000, 0xff80_0000, 0x7fa1_2345];
            for (i, value) in depth.iter_mut().enumerate() {
                *value = f32::from_bits(if i % 2 == 0 { boundaries[(i / 2) % boundaries.len()] } else { next(&mut seed) });
            }
            for keep_stencil in [false, true] {
                let source = keep_stencil.then_some(stencil.as_slice());
                let mut actual = raw.clone();
                actual.extend([0xa5; 17]);
                let mut expected = actual.clone();
                for y in 0..height {
                    for x in 0..width {
                        let linear = (y * width + x) as usize;
                        let at = tiled_offset(x, y, width) as usize * 4;
                        let value = (depth[linear].clamp(0.0, 1.0) * 0xFF_FFFF as f32) as u32;
                        let s = source.map_or(expected[at + 3], |s| s[linear]);
                        expected[at..at + 4].copy_from_slice(&(value | u32::from(s) << 24).to_le_bytes());
                    }
                }
                assert!(encode_depth(&depth, source, &mut actual, width));
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn unsupported_shapes_and_short_buffers_leave_destinations_for_scalar_fallback() {
        for (width, height) in [(0, 0), (7, 8), (8, 7), (9, 16), (16, 9)] {
            let pixels = width as usize * height as usize;
            let raw = vec![0; pixels * 4];
            let mut color = vec![0xa5; raw.len()];
            let mut depth = vec![-1.0; pixels];
            let mut stencil = vec![0xa5; pixels];
            let mut tiled = color.clone();
            assert!(!decode_color(&raw, &mut color, width));
            assert!(!encode_color(&raw, &mut tiled, width));
            assert!(!decode_depth(&raw, &mut depth, &mut stencil, width));
            assert!(!encode_depth(&depth, Some(&stencil), &mut tiled, width));
            assert!(color.iter().chain(&tiled).chain(&stencil).all(|&b| b == 0xa5));
            assert!(depth.iter().all(|&v| v == -1.0));
        }
        let mut color = vec![0xa5; 256];
        assert!(!decode_color(&[0; 255], &mut color, 8));
        assert!(!encode_color(&[0; 256], &mut color[..255], 8));
        let mut depth = [-1.0; 64];
        let mut stencil = [0xa5; 64];
        assert!(!decode_depth(&[0; 255], &mut depth, &mut stencil, 8));
        assert!(!decode_depth(&[0; 256], &mut depth, &mut stencil[..63], 8));
        assert!(!encode_depth(&depth, Some(&stencil[..63]), &mut color, 8));
        assert!(color.iter().chain(&stencil).all(|&b| b == 0xa5));
        assert_eq!(depth, [-1.0; 64]);
    }
}

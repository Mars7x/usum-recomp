//! Texture units 0-2: sampling a texture from its tiled form in memory.
//!
//! From 3dbrew "GPU/Internal Registers" (TEXUNITi_*): the dimensions,
//! the address (physical >> 3), the wrap modes and filters, the formats
//! (0 RGBA8, 1 RGB8, 2 RGBA5551, 3 RGB565, 4 RGBA4, 5 IA8, 6 HILO8, 7 I8,
//! 8 A8, 9 IA4, 10 I4, 11 A4, 12 ETC1, 13 ETC1A4), texels "laid out in
//! memory as a Z-order curve", and components "in reverse byte order".
//!
//! Not given by 3dbrew, so choices to check against the game's frames
//! (R-032): the 8x8 tiling and Z order used for framebuffers apply to
//! textures; memory row 0 is the texture's top (v = 1); 4-bit formats hold
//! the even texel in the low nibble; ETC1 blocks are 4x4 texels, four per
//! 8x8 tile in Z order, each a little-endian u64 (ETC1A4: the 4-bit
//! alphas first, column-major).

use std::sync::Arc;
use usum_guest::Memory;

use super::shader::Vec4;
use super::{tiled_offset, Pica};

/// Bits per texel of each format (ETC formats per texel, on average).
fn bits(format: u32) -> Option<u32> {
    Some(match format {
        0 => 32,
        1 => 24,
        2..=6 => 16,
        7..=9 => 8,
        10 | 11 => 4,
        12 => 4,
        13 => 8,
        _ => return None,
    })
}

/// One ETC1 4x4 block (the 64-bit word as the standard defines it) at
/// texel (x, y) of the block.
fn etc1(block: u64, x: u32, y: u32) -> [u8; 3] {
    const TABLE: [[i32; 4]; 8] = [
        [2, 8, -2, -8],
        [5, 17, -5, -17],
        [9, 29, -9, -29],
        [13, 42, -13, -42],
        [18, 60, -18, -60],
        [24, 80, -24, -80],
        [33, 106, -33, -106],
        [47, 183, -47, -183],
    ];
    let flip = block >> 32 & 1 == 1;
    let differential = block >> 33 & 1 == 1;
    let second = if flip { y >= 2 } else { x >= 2 };
    let base: [i32; 3] = if differential {
        let channel = |shift: u32| -> i32 {
            let c5 = ((block >> shift) & 0x1F) as i32;
            let d = (((block >> (shift - 3)) & 7) as i32) << 29 >> 29;
            let v = if second { c5 + d } else { c5 };
            (v << 3) | (v >> 2)
        };
        [channel(59), channel(51), channel(43)]
    } else {
        let channel = |shift: u32| -> i32 {
            let v = ((block >> (if second { shift - 4 } else { shift })) & 0xF) as i32;
            v * 0x11
        };
        [channel(60), channel(52), channel(44)]
    };
    let table = ((block >> (if second { 34 } else { 37 })) & 7) as usize;
    let index = x * 4 + y;
    let msb = (block >> (16 + index)) & 1;
    let lsb = (block >> index) & 1;
    let modifier = TABLE[table][(msb << 1 | lsb) as usize];
    base.map(|c| (c + modifier).clamp(0, 255) as u8)
}

/// The texel at (x, y) (y counting from the bottom) as RGBA, from the
/// texture's bytes.
fn texel(data: &[u8], width: u32, height: u32, format: u32, x: u32, y: u32) -> Option<[u8; 4]> {
    // rows in memory start from the top
    let row = height - 1 - y;
    let tile_bytes = 64 * bits(format)? / 8;
    let tile = (row / 8) * (width / 8) + x / 8;
    let tile_base = (tile * tile_bytes) as usize;
    if format >= 12 {
        let (bx, by) = ((x % 8) / 4, (row % 8) / 4);
        let block_bytes = if format == 13 { 16 } else { 8 };
        let at = tile_base + ((bx + 2 * by) * block_bytes) as usize;
        let read64 = |a: usize| -> Option<u64> { Some(u64::from_le_bytes(data.get(a..a + 8)?.try_into().ok()?)) };
        let (px, py) = (x % 4, row % 4);
        let alpha = if format == 13 {
            let a = read64(at)?;
            let nibble = ((a >> (4 * (px * 4 + py))) & 0xF) as u8;
            nibble * 0x11
        } else {
            0xFF
        };
        let color = read64(at + if format == 13 { 8 } else { 0 })?;
        let [r, g, b] = etc1(color, px, py);
        return Some([r, g, b, alpha]);
    }
    let z = tiled_offset(x % 8, row % 8, 8);
    let bits = bits(format)?;
    let at = tile_base + (z * bits / 8) as usize;
    let b = |k: usize| data.get(at + k).copied();
    let h = || Some(u16::from_le_bytes([b(0)?, b(1)?]));
    let x5 = |v: u16| ((v << 3) | (v >> 2)) as u8;
    let x4 = |v: u16| (v * 0x11) as u8;
    Some(match format {
        0 => [b(3)?, b(2)?, b(1)?, b(0)?],
        1 => [b(2)?, b(1)?, b(0)?, 0xFF],
        2 => {
            let v = h()?;
            [x5(v >> 11), x5((v >> 6) & 0x1F), x5((v >> 1) & 0x1F), if v & 1 == 1 { 0xFF } else { 0 }]
        }
        3 => {
            let v = h()?;
            [x5(v >> 11), (((v >> 5) & 0x3F) << 2 | ((v >> 5) & 0x3F) >> 4) as u8, x5(v & 0x1F), 0xFF]
        }
        4 => {
            let v = h()?;
            [x4(v >> 12), x4((v >> 8) & 0xF), x4((v >> 4) & 0xF), x4(v & 0xF)]
        }
        5 => {
            let (a, i) = (b(0)?, b(1)?);
            [i, i, i, a]
        }
        6 => [b(1)?, b(0)?, 0, 0xFF],
        7 => {
            let i = b(0)?;
            [i, i, i, 0xFF]
        }
        8 => [0, 0, 0, b(0)?],
        9 => {
            let v = b(0)?;
            let (i, a) = ((v >> 4) * 0x11, (v & 0xF) * 0x11);
            [i, i, i, a]
        }
        _ => {
            let byte = b(0)?;
            let nibble = if z.is_multiple_of(2) { byte & 0xF } else { byte >> 4 } * 0x11;
            if format == 10 {
                [nibble, nibble, nibble, 0xFF]
            } else {
                [0, 0, 0, nibble]
            }
        }
    })
}

/// Decoded textures kept across draws, by (address, format, width,
/// height). An entry is reused only while the texture's bytes in memory
/// are unchanged (compared against a copy), so textures the game rewrites
/// (render targets, streamed data) are decoded again.
#[derive(Debug, Default, Clone)]
pub struct Cache {
    entries: std::collections::HashMap<(u32, u32, u32, u32), Decoded>,
    pub hits: u64,
    pub misses: u64,
}

#[derive(Debug, Clone)]
struct Decoded {
    raw: Vec<u8>,
    /// RGBA, rows from the bottom (y = 0), as `texel` addresses them.
    /// Shared so an immutable Vulkan snapshot does not copy every texel.
    texels: Arc<[u8]>,
}

/// What a unit is configured with: address (virtual), format, size.
fn unit_key(p: &Pica, unit: usize) -> Option<(u32, u32, u32, u32)> {
    if p.regs[0x80] & (1 << unit) == 0 {
        return None;
    }
    let base_reg = [0x81, 0x91, 0x99][unit];
    let dim = p.regs[base_reg + 1];
    let format = p.regs[[0x8E, 0x96, 0x9E][unit]] & 0xF;
    let (width, height) = ((dim >> 16) & 0x7FF, dim & 0x7FF);
    if width == 0 || height == 0 {
        return None;
    }
    let address = p.pa_to_va((p.regs[base_reg + 4] & 0x0FFF_FFFF) << 3).ok()?;
    Some((address, format, width, height))
}

impl Cache {
    /// Makes sure unit `unit`'s texture is decoded and current.
    pub fn prepare(&mut self, p: &Pica, mem: &Memory, unit: usize) {
        let Some(key @ (address, format, width, height)) = unit_key(p, unit) else { return };
        let Some(size) = bits(format).map(|b| width * height * b / 8) else { return };
        let Some(raw) = mem.slice(address, size) else { return };
        if self.entries.get(&key).is_some_and(|d| d.raw == raw) {
            self.hits += 1;
            return;
        }
        self.misses += 1;
        if self.entries.len() > 512 {
            self.entries.clear();
        }
        let mut texels = Vec::with_capacity((width * height) as usize * 4);
        for y in 0..height {
            for x in 0..width {
                texels.extend_from_slice(&texel(raw, width, height, format, x, y).unwrap_or([0, 0, 0, 0xFF]));
            }
        }
        self.entries.insert(key, Decoded { raw: raw.to_vec(), texels: texels.into() });
    }
}

/// Owns one currently configured texture for the diagnostic Vulkan handoff.
/// Rows remain bottom-first, matching PICA's texture-coordinate convention
/// when uploaded to the host texture without changing vertex coordinates.
#[cfg(feature = "vulkan-compare")]
pub(crate) struct DecodedSnapshot {
    pub address: u32,
    pub source_size: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub parameters: u32,
    pub border: [u8; 4],
    pub pixels: Arc<[u8]>,
}

#[cfg(feature = "vulkan-compare")]
pub(crate) fn decoded_snapshot(p: &mut Pica, mem: &Memory, unit: usize) -> Option<DecodedSnapshot> {
    let key @ (address, format, width, height) = unit_key(p, unit)?;
    let source_size = width.checked_mul(height)?.checked_mul(bits(format)?)? / 8;
    let base_reg = [0x81, 0x91, 0x99][unit];
    let parameters = p.regs[base_reg + 2];
    let border = p.regs[base_reg].to_le_bytes();
    let snapshot = |decoded: &Decoded| DecodedSnapshot {
        address,
        source_size,
        format,
        width,
        height,
        parameters,
        border,
        pixels: Arc::clone(&decoded.texels),
    };

    // Populate the same validated cache used by software rasterization. This
    // also gives a Vulkan-only list stable Arc identities across draws.
    let mut cache = std::mem::take(&mut p.texture_cache);
    cache.prepare(p, mem, unit);
    let result = cache.entries.get(&key).map(snapshot);
    p.texture_cache = cache;
    result
}

/// A texture unit's configuration and decoded texels, resolved once per
/// draw.
pub struct Bound<'a> {
    texels: &'a [u8],
    width: u32,
    height: u32,
    wrap_s: u32,
    wrap_t: u32,
    linear: bool,
    border: Vec4,
}

fn unpack(c: u32) -> Vec4 {
    [0, 8, 16, 24].map(|s| ((c >> s) & 0xFF) as f32 / 255.0)
}

impl<'a> Bound<'a> {
    /// Texture unit `unit` (0-2) as configured now, or None if disabled
    /// or not readable (it then samples as (0, 0, 0, 1)).
    /// Call `Cache::prepare` for the unit first.
    pub fn new(p: &Pica, cache: &'a Cache, unit: usize) -> Option<Bound<'a>> {
        let key @ (_, _, width, height) = unit_key(p, unit)?;
        let base_reg = [0x81, 0x91, 0x99][unit];
        let param = p.regs[base_reg + 2];
        let texels = &cache.entries.get(&key)?.texels;
        Some(Bound {
            texels,
            width,
            height,
            wrap_s: (param >> 12) & 7,
            wrap_t: (param >> 8) & 7,
            linear: (param >> 1) & 1 == 1,
            border: unpack(p.regs[base_reg]),
        })
    }

    /// Coordinate `c` wrapped into 0..size by `mode`; None for a texel
    /// outside a clamp-to-border texture.
    #[inline(always)]
    fn wrap(c: i64, size: u32, mode: u32) -> Option<u32> {
        let s = i64::from(size);
        match mode {
            0 => Some(c.clamp(0, s - 1) as u32),
            1 => (0..s).contains(&c).then_some(c as u32),
            // the same as rem_euclid for a power of two
            2 if size.is_power_of_two() => Some((c & (s - 1)) as u32),
            2 => Some(c.rem_euclid(s) as u32),
            _ => {
                let m = c.rem_euclid(2 * s);
                Some(if m < s { m } else { 2 * s - 1 - m } as u32)
            }
        }
    }

    /// The texel at wrapped (x, y), or the border color.
    #[inline(always)]
    fn texel(&self, x: Option<u32>, y: Option<u32>) -> Vec4 {
        match (x, y) {
            (Some(x), Some(y)) => {
                let at = (y * self.width + x) as usize * 4;
                let c = &self.texels[at..at + 4];
                super::arr4(|k| UNIT[usize::from(c[k])])
            }
            _ => self.border,
        }
    }

    /// Samples at (u, v): nearest or bilinear, with the wrap modes. Each
    /// coordinate is wrapped once (bilinear reads two columns and two
    /// rows).
    pub fn sample(&self, u: f32, v: f32) -> Vec4 {
        let (w, h) = (self.width as f32, self.height as f32);
        let wrap_x = |c: i64| Self::wrap(c, self.width, self.wrap_s);
        let wrap_y = |c: i64| Self::wrap(c, self.height, self.wrap_t);
        if !self.linear {
            return self.texel(wrap_x(floor(u * w) as i64), wrap_y(floor(v * h) as i64));
        }
        let (fx, fy) = (u * w - 0.5, v * h - 0.5);
        let (x0, y0) = (floor(fx), floor(fy));
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let (xa, xb, ya, yb) = (wrap_x(x0), wrap_x(x0 + 1), wrap_y(y0), wrap_y(y0 + 1));
        let (a, b, c, d) = (self.texel(xa, ya), self.texel(xb, ya), self.texel(xa, yb), self.texel(xb, yb));
        super::arr4(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        })
    }
}

/// c / 255 for every byte, as computed at run time (IEEE division in a
/// constant is the same operation).
pub(super) static UNIT: [f32; 256] = {
    let mut t = [0.0; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = i as f32 / 255.0;
        i += 1;
    }
    t
};

/// `x.floor()` without the library call the baseline x86-64 target makes
/// for it: the same value for every finite x within i64 and for NaN (0
/// either way once cast); beyond that both saturate the same when cast.
fn floor(x: f32) -> f32 {
    if x.abs() >= 8_388_608.0 || x.is_nan() {
        return x.floor();
    }
    let t = x as i32 as f32;
    if t > x {
        t - 1.0
    } else {
        // t has x's sign except when it is zero: keep -0.0 as -0.0
        t.copysign(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "vulkan-compare")]
    use usum_guest::Perm;

    #[test]
    fn floor_and_unit_match_the_library() {
        for x in [-2.5f32, -1.0, -0.5, -0.0, 0.0, 0.25, 1.0, 3.999, 8_388_607.5, -8_388_607.5, 1e9, -1e9, f32::INFINITY] {
            assert_eq!(floor(x).to_bits(), x.floor().to_bits(), "{x}");
        }
        assert!(floor(f32::NAN).is_nan());
        for c in 0..=255u8 {
            assert_eq!(UNIT[usize::from(c)].to_bits(), (f32::from(c) / 255.0).to_bits());
        }
    }

    #[test]
    fn etc1_individual_and_differential_blocks() {
        // individual mode, both halves base 0x8 (0x88), table 0, all
        // indices 0 (modifier +2)
        let block: u64 = (0x88u64 << 56) | (0x88u64 << 48) | (0x88u64 << 40);
        assert_eq!(etc1(block, 0, 0), [0x8A, 0x8A, 0x8A]);
        // differential: base 16 (5-bit) for the first half
        let diff: u64 = (16u64 << 59) | (16u64 << 51) | (16u64 << 43) | (1 << 33);
        assert_eq!(etc1(diff, 0, 0), [0x86, 0x86, 0x86]);
    }

    #[cfg(feature = "vulkan-compare")]
    #[test]
    fn vulkan_snapshot_shares_a_current_software_decode() {
        let mut pica = Pica::default();
        pica.regs[0x80] = 1;
        pica.regs[0x82] = 8 << 16 | 8;
        pica.regs[0x85] = 0x2000_0000 >> 3;
        pica.regs[0x8E] = 0;
        let mut memory = Memory::new();
        memory.map(0x3000_0000, vec![0x5A; 8 * 8 * 4], Perm::RW).unwrap();

        let mut cache = std::mem::take(&mut pica.texture_cache);
        cache.prepare(&pica, &memory, 0);
        pica.texture_cache = cache;
        let first = decoded_snapshot(&mut pica, &memory, 0).unwrap();
        let second = decoded_snapshot(&mut pica, &memory, 0).unwrap();

        assert!(Arc::ptr_eq(&first.pixels, &second.pixels));
        assert!(Arc::ptr_eq(&first.pixels, &pica.texture_cache.entries.values().next().unwrap().texels));
    }
}

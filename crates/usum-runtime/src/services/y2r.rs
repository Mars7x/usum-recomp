//! `y2r:u`: the YUV-to-RGB converter, used for video frames. Command IDs
//! per 3dbrew ("Camera Services", Y2R). 3dbrew gives no behavior beyond
//! the table; the conversion itself (fixed-point coefficients, 8-row
//! strips, 8x8 blocks in Z order, the transfer units and gaps of the two
//! DMA ends) follows Citra's HLE, read as documentation (R-050). The
//! conversion runs at StartConversion, whole, and then reports itself
//! finished.

use usum_guest::memory::Memory;

/// One DMA end: a buffer in the process, its total size, and how it is
/// walked: `unit` bytes, then `gap` bytes skipped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dma {
    pub address: u32,
    pub size: u32,
    pub unit: u32,
    pub gap: u32,
}

impl Dma {
    fn from_words(w: &[u32]) -> Dma {
        Dma { address: w[0], size: w[1], unit: w[2] & 0xFFFF, gap: w[3] & 0xFFFF }
    }

    /// The buffer's bytes in walking order.
    fn gather(&self, mem: &Memory) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(self.size as usize);
        let unit = if self.unit == 0 { self.size } else { self.unit };
        let mut at = self.address;
        while (out.len() as u32) < self.size {
            let n = unit.min(self.size - out.len() as u32);
            out.extend(mem.read_range(at, n).map_err(|f| format!("Y2R input: {f}"))?);
            at = at.wrapping_add(n + self.gap);
        }
        Ok(out)
    }

    /// Writes `bytes` in walking order (at most `size` of them).
    fn scatter(&self, mem: &mut Memory, bytes: &[u8]) -> Result<(), String> {
        let unit = if self.unit == 0 { self.size } else { self.unit } as usize;
        let mut at = self.address;
        for chunk in bytes[..bytes.len().min(self.size as usize)].chunks(unit.max(1)) {
            mem.device_write(at, chunk).map_err(|f| format!("Y2R output: {f}"))?;
            at = at.wrapping_add(chunk.len() as u32 + self.gap);
        }
        Ok(())
    }
}

/// Citra's standard coefficient sets: ITU-R BT.601, BT.709, and the two
/// with video range scaling.
const STANDARD: [[i32; 8]; 4] = [
    [0x100, 0x166, 0xB6, 0x58, 0x1C5, -0x166F, 0x10EE, -0x1C5B],
    [0x100, 0x193, 0x77, 0x2F, 0x1DB, -0x1933, 0xA7C, -0x1D51],
    [0x12A, 0x198, 0xD0, 0x64, 0x204, -0x1BDE, 0x10F2, -0x229B],
    [0x12A, 0x1CA, 0x88, 0x36, 0x21C, -0x1F04, 0x99C, -0x2421],
];

/// The converter's settings and buffers.
#[derive(Debug, Clone, Default)]
pub struct Y2r {
    pub input_format: u32,
    pub output_format: u32,
    pub rotation: u32,
    pub block_alignment: u32,
    pub spacial_dithering: u32,
    pub temporal_dithering: u32,
    pub transfer_end_interrupt: u32,
    pub line_width: u32,
    pub lines: u32,
    pub coefficients: [i32; 8],
    pub standard: u32,
    pub alpha: u32,
    pub dithering_weights: [u32; 8],
    pub y: Dma,
    pub u: Dma,
    pub v: Dma,
    pub yuv: Dma,
    pub receiving: Dma,
    /// The transfer-end event, once asked for.
    pub event: Option<u32>,
}

/// What the kernel must do for a request beyond replying.
pub enum Action {
    Reply(Vec<u32>),
    /// Make the transfer-end event (reply with it).
    Event,
    /// Convert now, then signal the event.
    Convert,
}

impl Y2r {
    /// Handles request `w` (header first); None if not known.
    pub fn request(&mut self, w: &[u32]) -> Option<Action> {
        let command = (w[0] >> 16) as u16;
        let ok = || Action::Reply(super::result_reply(command));
        let value = |v: u32| Action::Reply(vec![super::header(command, 2, 0), 0, v]);
        Some(match w[0] {
            0x0001_0040 => {
                self.input_format = w[1];
                ok()
            }
            0x0002_0000 => value(self.input_format),
            0x0003_0040 => {
                self.output_format = w[1];
                ok()
            }
            0x0004_0000 => value(self.output_format),
            0x0005_0040 => {
                self.rotation = w[1];
                ok()
            }
            0x0006_0000 => value(self.rotation),
            0x0007_0040 => {
                self.block_alignment = w[1];
                ok()
            }
            0x0008_0000 => value(self.block_alignment),
            0x0009_0040 => {
                self.spacial_dithering = w[1];
                ok()
            }
            0x000A_0000 => value(self.spacial_dithering),
            0x000B_0040 => {
                self.temporal_dithering = w[1];
                ok()
            }
            0x000C_0000 => value(self.temporal_dithering),
            0x000D_0040 => {
                self.transfer_end_interrupt = w[1];
                ok()
            }
            0x000E_0000 => value(self.transfer_end_interrupt),
            0x000F_0000 => Action::Event,
            // SetSendingY/U/V/Yuv: address, size, unit, gap, process handle
            0x0010_0102 => {
                self.y = Dma::from_words(&w[1..5]);
                ok()
            }
            0x0011_0102 => {
                self.u = Dma::from_words(&w[1..5]);
                ok()
            }
            0x0012_0102 => {
                self.v = Dma::from_words(&w[1..5]);
                ok()
            }
            0x0013_0102 => {
                self.yuv = Dma::from_words(&w[1..5]);
                ok()
            }
            // IsFinishedSending*/Receiving: the conversion is done at once
            0x0014_0000..=0x0017_0000 | 0x0019_0000 if w[0] & 0xFFFF == 0 => value(1),
            0x0018_0102 => {
                self.receiving = Dma::from_words(&w[1..5]);
                ok()
            }
            0x001A_0040 => {
                self.line_width = w[1];
                ok()
            }
            0x001B_0000 => value(self.line_width),
            0x001C_0040 => {
                self.lines = w[1];
                ok()
            }
            0x001D_0000 => value(self.lines),
            0x001E_0100 => {
                for (k, c) in self.coefficients.iter_mut().enumerate() {
                    *c = i32::from((w[1 + k / 2] >> (16 * (k % 2))) as u16 as i16);
                }
                ok()
            }
            0x0020_0040 => {
                let set = STANDARD.get(w[1] as usize)?;
                self.standard = w[1];
                self.coefficients = *set;
                ok()
            }
            0x0022_0040 => {
                self.alpha = w[1];
                ok()
            }
            0x0023_0000 => value(self.alpha),
            0x0024_0200 => {
                self.dithering_weights.copy_from_slice(&w[1..9]);
                ok()
            }
            0x0026_0000 => Action::Convert,
            0x0027_0000 => ok(),
            0x0028_0000 => value(0),
            // SetPackageParameter: the common settings in one go (Citra's
            // layout: formats, rotation, block alignment as bytes; line
            // width and lines as halfwords; standard coefficient, alpha)
            0x0029_01C0 => {
                let b = |i: usize| (w[1 + i / 4] >> (8 * (i % 4))) & 0xFF;
                self.input_format = b(0);
                self.output_format = b(1);
                self.rotation = b(2);
                self.block_alignment = b(3);
                self.line_width = w[2] & 0xFFFF;
                self.lines = w[2] >> 16;
                let standard = w[3] & 0xFF;
                if let Some(set) = STANDARD.get(standard as usize) {
                    self.standard = standard;
                    self.coefficients = *set;
                }
                self.alpha = w[4] & 0xFFFF;
                ok()
            }
            0x002A_0000 => value(0),
            0x002B_0000 | 0x002C_0000 => ok(),
            _ => return None,
        })
    }

    /// Converts the sent YUV into the receiving buffer.
    pub fn convert(&self, mem: &mut Memory) -> Result<(), String> {
        let (width, lines) = (self.line_width, self.lines);
        if width == 0 || lines == 0 || width % 8 != 0 || width > 1024 || lines > 1024 {
            return Err(format!("Y2R conversion of {width}x{lines}"));
        }
        if self.rotation != 0 {
            return Err(format!("Y2R rotation {}", self.rotation));
        }
        let (w, h) = (width as usize, lines as usize);
        // input planes as Y, U, V sample arrays
        let sample = |bytes: &[u8], i: usize, wide: bool| -> i32 {
            if wide {
                i32::from(bytes.get(2 * i).copied().unwrap_or(0))
            } else {
                i32::from(bytes.get(i).copied().unwrap_or(0))
            }
        };
        let (planar, wide, half_lines) = match self.input_format {
            0 => (true, false, false),
            1 => (true, false, true),
            2 => (true, true, false),
            3 => (true, true, true),
            4 => (false, false, false),
            other => return Err(format!("Y2R input format {other}")),
        };
        let (yb, ub, vb) = if planar {
            (self.y.gather(mem)?, self.u.gather(mem)?, self.v.gather(mem)?)
        } else {
            (self.yuv.gather(mem)?, Vec::new(), Vec::new())
        };
        let c = &self.coefficients;
        let output = match self.output_format {
            0 => 0,
            1 => 1,
            2 => 3,
            3 => 2,
            other => return Err(format!("Y2R output format {other}")),
        };
        let bpp = crate::pica::bytes_per_pixel(output).ok_or("Y2R output format")?;
        let alpha = (self.alpha & 0xFF) as u8;
        let mut out = vec![0u8; w * h * bpp];
        for y in 0..h {
            for x in 0..w {
                let (yy, u, v) = if planar {
                    let chroma = if half_lines { (y / 2) * (w / 2) + x / 2 } else { y * (w / 2) + x / 2 };
                    (sample(&yb, y * w + x, wide), sample(&ub, chroma, wide), sample(&vb, chroma, wide))
                } else {
                    let base = (y * w + (x / 2) * 2) * 2;
                    (sample(&yb, (y * w + x) * 2, false), sample(&yb, base + 1, false), sample(&yb, base + 3, false))
                };
                let cy = c[0] * yy;
                let r = ((cy + c[1] * v) >> 3) + c[5] + 0x18;
                let g = ((cy - c[2] * v - c[3] * u) >> 3) + c[6] + 0x18;
                let b = ((cy + c[4] * u) >> 3) + c[7] + 0x18;
                let channel = |v: i32| (v >> 5).clamp(0, 0xFF) as u8;
                let px = crate::pica::encode_pixel(output, [channel(r), channel(g), channel(b), alpha]);
                // linear rows, or 8x8 blocks of each 8-row strip in Z order
                let index = if self.block_alignment == 1 {
                    let (tx, px_, py) = (x / 8, x % 8, y % 8);
                    let z = (px_ & 1) | (py & 1) << 1 | (px_ & 2) << 1 | (py & 2) << 2 | (px_ & 4) << 2 | (py & 4) << 3;
                    (y / 8) * (w * 8) + tx * 64 + z
                } else {
                    y * w + x
                };
                out[index * bpp..index * bpp + bpp].copy_from_slice(&px);
            }
        }
        self.receiving.scatter(mem, &out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_and_colors_convert_to_their_rgb() {
        let c = STANDARD[0];
        let rgb = |yy: i32, u: i32, v: i32| {
            let cy = c[0] * yy;
            let ch = |x: i32| (x >> 5).clamp(0, 0xFF);
            (
                ch(((cy + c[1] * v) >> 3) + c[5] + 0x18),
                ch(((cy - c[2] * v - c[3] * u) >> 3) + c[6] + 0x18),
                ch(((cy + c[4] * u) >> 3) + c[7] + 0x18),
            )
        };
        // mid grey stays grey, near white stays white (BT.601, full range)
        let (r, g, b) = rgb(128, 128, 128);
        assert!((r - 128).abs() <= 2 && (g - 128).abs() <= 2 && (b - 128).abs() <= 2, "{r} {g} {b}");
        let (r, g, b) = rgb(255, 128, 128);
        assert!(r >= 250 && g >= 250 && b >= 250, "{r} {g} {b}");
        // strong V is red, strong U is blue
        let (r, _, b) = rgb(128, 128, 255);
        assert!(r > 200 && b < 140);
        let (r, _, b) = rgb(128, 255, 128);
        assert!(b > 200 && r < 140);
    }

    #[test]
    fn package_parameters_and_coefficients() {
        let mut y = Y2r::default();
        // input 1, output 0, rotation 0, block 1; 320 x 240; standard 2; alpha 0xFF
        y.request(&[0x0029_01C0, 1 | 1 << 24, 320 | 240 << 16, 2, 0xFF, 0, 0, 0]);
        assert_eq!((y.input_format, y.block_alignment, y.line_width, y.lines, y.alpha), (1, 1, 320, 240, 0xFF));
        assert_eq!(y.coefficients, STANDARD[2]);
        y.request(&[0x001E_0100, 0xFFFF_0001, 2, 3, 4]);
        assert_eq!(y.coefficients[..3], [1, -1, 2]);
    }
}

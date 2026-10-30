//! Small host-only bitmap text for testing counters and system dialogs.
//! No game fonts, textures or readbacks.

pub(super) struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

pub(super) const ORIGIN: u32 = 8;
const BACKGROUND: [u8; 3] = [16, 24, 32];
const INK: [u8; 3] = [128, 255, 160];

fn glyph(ch: char) -> [u8; 7] {
    match ch {
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'G' => [14, 17, 16, 23, 17, 17, 14],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'N' => [17, 25, 25, 21, 19, 19, 17],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        _ => [0; 7],
    }
}

impl Bitmap {
    pub fn new(text: &str) -> Self {
        let lines: Vec<_> = text.lines().collect();
        let width = lines.iter().map(|line| line.chars().count()).max().unwrap_or(0) as u32 * 12 + 8;
        let height = lines.len() as u32 * 18 + 8;
        let mut rgb: Vec<_> = (0..width * height).flat_map(|_| BACKGROUND).collect();
        for (line, text) in lines.iter().enumerate() {
            for (column, ch) in text.chars().enumerate() {
                for (y, row) in glyph(ch).into_iter().enumerate() {
                    for x in 0..5 {
                        if row & (1 << (4 - x)) == 0 {
                            continue;
                        }
                        for dy in 0..2 {
                            for dx in 0..2 {
                                let px = 4 + column as u32 * 12 + x * 2 + dx;
                                let py = 4 + line as u32 * 18 + y as u32 * 2 + dy;
                                let at = ((py * width + px) * 3) as usize;
                                rgb[at..at + 3].copy_from_slice(&INK);
                            }
                        }
                    }
                }
            }
        }
        Self { width, height, rgb }
    }

    pub fn composite(&self, output: &mut [u32], width: u32, height: u32) {
        for y in 0..self.height.min(height.saturating_sub(ORIGIN)) {
            for x in 0..self.width.min(width.saturating_sub(ORIGIN)) {
                let at = ((y * self.width + x) * 3) as usize;
                let [r, g, b]: [u8; 3] = self.rgb[at..at + 3].try_into().unwrap();
                output[((ORIGIN + y) * width + ORIGIN + x) as usize] = u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_counter_clips_without_touching_game_pixels_outside_the_panel() {
        let bitmap = Bitmap::new("FPS 30.0\nWIN 60.0");
        assert_eq!((bitmap.width, bitmap.height), (104, 44));
        assert_eq!(&bitmap.rgb[..3], &BACKGROUND);
        assert_eq!(&bitmap.rgb[((4 * bitmap.width + 4) * 3) as usize..][..3], &INK);
        for (width, height) in [(0, 0), (3, 7), (9, 9), (23, 21), (160, 80)] {
            let mut output = vec![0x123456; (width * height) as usize];
            bitmap.composite(&mut output, width, height);
            for y in 0..height {
                for x in 0..width {
                    let pixel = output[(y * width + x) as usize];
                    if x < ORIGIN || y < ORIGIN || x >= ORIGIN + bitmap.width || y >= ORIGIN + bitmap.height {
                        assert_eq!(pixel, 0x123456);
                    } else {
                        let at = (((y - ORIGIN) * bitmap.width + x - ORIGIN) * 3) as usize;
                        assert_eq!(
                            pixel,
                            u32::from(bitmap.rgb[at]) << 16 | u32::from(bitmap.rgb[at + 1]) << 8 | u32::from(bitmap.rgb[at + 2])
                        );
                    }
                }
            }
        }
    }
}

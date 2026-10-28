//! "Bottom LZ": the backwards LZSS variant the 3DS SDK compresses ExeFS
//! `.code` with (exheader flag), decompressed by the loader in place.
//!
//! Format (3dbrew "ExeFS"; ctrtool's `lzss_decompress` is the reference
//! the community uses): the last 8 bytes are a footer,
//!
//! - `u32` at end-8: bits 0..24 = how far before the end the compressed
//!   region starts, bits 24..32 = footer length (footer and padding);
//! - `u32` at end-4: decompressed size minus compressed size.
//!
//! Bytes before the compressed region are stored as-is. The region is read
//! backwards from the footer: a control byte, then for each of its bits
//! from the MSB, a literal byte (0) or a 2-byte back reference (1) whose
//! top nibble + 3 is the length and low 12 bits + 2 the distance. Output is
//! written backwards from the end.

use crate::Error;

const WHAT: &str = "compressed code";
const FOOTER_SIZE: usize = 8;
const MIN_RUN: usize = 3;
const MIN_DISTANCE: usize = 2;

pub fn decompress(input: &[u8], max_output: usize) -> Result<Vec<u8>, Error> {
    let size = input.len();
    if size < FOOTER_SIZE {
        return Err(Error::malformed(WHAT, "shorter than its footer"));
    }
    let word = |at: usize| u32::from_le_bytes(input[at..at + 4].try_into().expect("4 bytes"));
    let footer = word(size - 8);
    let extra = word(size - 4) as usize;
    let footer_len = (footer >> 24) as usize;
    let region = (footer & 0x00FF_FFFF) as usize;
    if footer_len < FOOTER_SIZE || footer_len > region || region > size {
        return Err(Error::malformed(WHAT, format!("footer {footer:#010x} does not fit {size} bytes")));
    }
    let out_size = size.checked_add(extra).filter(|&n| n <= max_output).ok_or_else(|| {
        Error::malformed(WHAT, format!("claims {extra} more bytes than stored, beyond the {max_output}-byte limit"))
    })?;

    let mut out = vec![0u8; out_size];
    out[..size].copy_from_slice(input);
    let stop = size - region;
    let mut index = size - footer_len;
    let mut cursor = out_size;
    while index > stop {
        index -= 1;
        let mut control = input[index];
        for _ in 0..8 {
            if index <= stop || cursor == 0 {
                break;
            }
            if control & 0x80 != 0 {
                if index < stop + 2 {
                    return Err(Error::malformed(WHAT, "a back reference is cut short"));
                }
                index -= 2;
                let pair = u16::from_le_bytes([input[index], input[index + 1]]) as usize;
                let length = (pair >> 12) + MIN_RUN;
                let distance = (pair & 0xFFF) + MIN_DISTANCE;
                if cursor < length {
                    return Err(Error::malformed(WHAT, "a back reference runs past the start"));
                }
                for _ in 0..length {
                    let byte = *out
                        .get(cursor + distance)
                        .ok_or_else(|| Error::malformed(WHAT, "a back reference reads past the end"))?;
                    cursor -= 1;
                    out[cursor] = byte;
                }
            } else {
                index -= 1;
                cursor -= 1;
                out[cursor] = input[index];
            }
            control <<= 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "XYZ" x 8, encoded by hand: three literals (Z, Y, X, read
    /// backwards), then an 18-byte run and a 3-byte run. Control byte
    /// 0b0001_1000. A run's distance is taken before the cursor moves, so a
    /// distance field of 0 (meaning 2) repeats with period 3.
    const XYZ8: [u8; 16] = [
        0x00, 0x00, // second run: length 3
        0x00, 0xF0, // first run: length 18
        b'X', b'Y', b'Z', 0x18, // literals and the control byte
        0x10, 0x00, 0x00, 0x08, // region 16 bytes, footer 8 bytes
        0x08, 0x00, 0x00, 0x00, // 8 bytes more when decompressed
    ];

    #[test]
    fn decodes_hand_built_vector() {
        assert_eq!(decompress(&XYZ8, 1 << 20).unwrap(), b"XYZ".repeat(8));
    }

    #[test]
    fn keeps_uncompressed_prefix() {
        let mut input = b"head".to_vec();
        input.extend_from_slice(&XYZ8);
        let out = decompress(&input, 1 << 20).unwrap();
        assert_eq!(&out[..4], b"head");
        assert_eq!(&out[4..], b"XYZ".repeat(8).as_slice());
    }

    #[test]
    fn rejects_bad_footers_and_limits() {
        assert!(decompress(&[0; 4], 100).is_err());
        let mut bad = XYZ8;
        bad[8] = 0xFF; // region larger than the input
        assert!(decompress(&bad, 1 << 20).is_err());
        assert!(decompress(&XYZ8, 23).is_err(), "output limit must hold");
    }

    /// Mutated inputs may fail, but must never panic or overrun.
    #[test]
    fn survives_mutations() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..20_000 {
            let mut input = XYZ8;
            for _ in 0..3 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                input[(state % 16) as usize] = (state >> 32) as u8;
            }
            let _ = decompress(&input, 1 << 16);
        }
    }
}

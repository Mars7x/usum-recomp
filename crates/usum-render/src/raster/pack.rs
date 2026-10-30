//! Native-endian vertex upload bytes, preserving every float's bit pattern.
use std::{borrow::Cow, sync::OnceLock};

pub(super) fn vertex_inputs(inputs: &[[[f32; 4]; 16]]) -> Cow<'_, [u8]> {
    static BLOCKS: OnceLock<bool> = OnceLock::new();
    static BORROW: OnceLock<bool> = OnceLock::new();
    prepare(
        inputs,
        *BORROW.get_or_init(|| std::env::var("USUM_VULKAN_BORROW_INPUTS").as_deref() != Ok("0")),
        *BLOCKS.get_or_init(|| std::env::var("USUM_VULKAN_BLOCK_PACK").as_deref() != Ok("0")),
    )
}

fn prepare(inputs: &[[[f32; 4]; 16]], borrow: bool, blocks: bool) -> Cow<'_, [u8]> {
    if borrow && blocks {
        // Arrays of f32 have no padding. Keep the original native-endian bits,
        // and borrow only until upload_draw_buffer has copied the bytes.
        Cow::Borrowed(bytemuck::cast_slice(inputs))
    } else {
        Cow::Owned(encode(inputs, blocks))
    }
}

fn encode(inputs: &[[[f32; 4]; 16]], blocks: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(inputs.len() * 256);
    if blocks {
        for input in inputs {
            let packed = input.map(|register| register.map(f32::to_ne_bytes));
            bytes.extend_from_slice(packed.as_flattened().as_flattened());
        }
    } else {
        for input in inputs {
            for register in input {
                for value in register {
                    bytes.extend_from_slice(&value.to_ne_bytes());
                }
            }
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_blocks_match_scalar_uploads_for_varied_float_bits_and_sizes() {
        let mut seed = 0x91827364u64;
        for count in [0, 1, 3, 4, 16, 257] {
            let inputs: Vec<[[f32; 4]; 16]> = (0..count)
                .map(|_| {
                    std::array::from_fn(|_| {
                        std::array::from_fn(|_| {
                            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                            f32::from_bits((seed >> 32) as u32)
                        })
                    })
                })
                .collect();
            let scalar = encode(&inputs, false);
            let blocks = encode(&inputs, true);
            assert_eq!(blocks.len(), count * 256);
            assert_eq!(blocks, scalar);
            let borrowed = prepare(&inputs, true, true);
            assert!(matches!(borrowed, Cow::Borrowed(_)));
            assert_eq!(borrowed.as_ref(), scalar);
            assert_eq!(borrowed.as_ptr(), inputs.as_ptr().cast::<u8>());
            for flags in [(false, true), (false, false), (true, false)] {
                let fallback = prepare(&inputs, flags.0, flags.1);
                assert!(matches!(fallback, Cow::Owned(_)));
                assert_eq!(fallback.as_ref(), scalar);
            }
            for (word, value) in blocks.as_chunks::<4>().0.iter().zip(inputs.iter().flatten().flatten()) {
                assert_eq!(*word, value.to_bits().to_ne_bytes());
            }
        }
    }

    #[test]
    fn blocks_preserve_signed_zero_nan_payloads_and_register_order() {
        let bits = [0, 0x8000_0000, 1, 0x007f_ffff, 0x0080_0000, 0x7f80_0000, 0xff80_0000, 0x7fc1_2345, 0x7fa1_2345, 0xffc5_4321];
        let input = std::array::from_fn(|r| std::array::from_fn(|c| f32::from_bits(bits[(r * 4 + c) % bits.len()])));
        assert_eq!(encode(&[input], true), encode(&[input], false));
        #[repr(C)]
        struct Offset {
            prefix: u32,
            input: [[f32; 4]; 16],
        }
        let offset = Offset { prefix: 7, input };
        assert_eq!(encode(std::slice::from_ref(&offset.input), true), encode(&[input], false));
        let view = prepare(std::slice::from_ref(&offset.input), true, true);
        assert!(matches!(view, Cow::Borrowed(_)));
        assert_eq!(view.as_ptr(), std::ptr::from_ref(&offset.input).cast::<u8>());
        assert_eq!(view.as_ref(), encode(&[input], false));
    }
}

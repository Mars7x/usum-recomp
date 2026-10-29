//! Native-endian vertex upload bytes, preserving every float's bit pattern.
use std::sync::OnceLock;

pub(super) fn vertex_inputs(inputs: &[[[f32; 4]; 16]]) -> Vec<u8> {
    static BLOCKS: OnceLock<bool> = OnceLock::new();
    encode(inputs, *BLOCKS.get_or_init(|| std::env::var("USUM_VULKAN_BLOCK_PACK").as_deref() != Ok("0")))
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
    }
}

//! Aligned ARM register transfers. Only a completely proven ordinary-RAM
//! span bypasses the scalar accessors; faults retain their order and effects.
use super::*;
use crate::ops::{read_aligned, write_aligned};

impl Memory {
    /// Read an LDM register list in address order. No registers are committed
    /// by this helper; the caller applies writeback only after every read.
    pub fn read_aligned_words<const N: usize>(&self, address: u32) -> Result<[u32; N], Fault> {
        assert!(N <= 16, "ARM register list exceeds sixteen words");
        if N == 0 {
            return Ok([0; N]);
        }
        if address & 3 != 0 {
            return Err(Fault::Unaligned { address });
        }
        if N >= 4 {
            if let Some((region, offset)) = self.block_location(address, N * 4, false) {
                let words = self.regions[region].bytes[offset..offset + N * 4].as_chunks::<4>().0;
                return Ok(std::array::from_fn(|index| u32::from_le_bytes(words[index])));
            }
        }
        self.read_block_reference(address)
    }

    /// Store an STM register list. Fallback writes retain earlier complete
    /// words when a later word faults; this is not an atomic range write.
    pub fn write_aligned_words<const N: usize>(&mut self, address: u32, values: [u32; N]) -> Result<(), Fault> {
        assert!(N <= 16, "ARM register list exceeds sixteen words");
        if N == 0 {
            return Ok(());
        }
        if address & 3 != 0 {
            return Err(Fault::Unaligned { address });
        }
        if N >= 4 {
            if let Some((region, offset)) = self.block_location(address, N * 4, true) {
                let region = &mut self.regions[region];
                if region.logical.get().is_some() {
                    Self::clear_word_logical(region);
                }
                for (word, value) in region.bytes[offset..offset + N * 4].as_chunks_mut::<4>().0.iter_mut().zip(values) {
                    *word = value.to_le_bytes();
                }
                return Ok(());
            }
        }
        self.write_block_reference(address, values)
    }

    // One lookup per entire transfer, kept out of generated call sites.
    #[inline(never)]
    fn block_location(&self, address: u32, len: usize, write: bool) -> Option<(usize, usize)> {
        if !self.block_words || !self.inline_words || !self.deferred.span_uncovered(address, len) {
            return None;
        }
        let entry = self.page_index.as_ref()?.lookup(address);
        if entry == index::UNMAPPED || entry == index::PARTIAL {
            return None;
        }
        let index = (entry - 1) as usize;
        let region = &self.regions[index];
        let offset = address.wrapping_sub(region.base) as usize;
        let permitted = if write { region.perm.write } else { region.perm.read };
        (permitted && offset.checked_add(len).is_some_and(|end| end <= region.bytes.len())).then_some((index, offset))
    }

    #[inline(never)]
    fn read_block_reference<const N: usize>(&self, address: u32) -> Result<[u32; N], Fault> {
        let mut words = [0; N];
        for (index, word) in words.iter_mut().enumerate() {
            *word = read_aligned(self, address.wrapping_add(index as u32 * 4))?;
        }
        Ok(words)
    }

    #[inline(never)]
    fn write_block_reference<const N: usize>(&mut self, address: u32, values: [u32; N]) -> Result<(), Fault> {
        for (index, value) in values.into_iter().enumerate() {
            write_aligned(self, address.wrapping_add(index as u32 * 4), value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapped() -> Memory {
        let mut memory = Memory { block_words: true, inline_words: true, use_page_index: true, ..Memory::new() };
        for (address, size, perm) in [
            (0, 4096, Perm::RW),
            (0x4000, 10, Perm::RW),
            (0x400a, 10, Perm::R),
            (0x4014, 12, Perm::RW),
            (0x8000, 4096, Perm { read: false, write: true }),
            (0xa000, 4096, Perm::R),
            (0x10000, 8192, Perm::RW),
            (0xffff_f000, 4096, Perm::RW),
        ] {
            memory.map(address, (0..size).map(|n| (n * 73 + 11) as u8).collect(), perm).unwrap();
        }
        memory
    }

    fn reference_read<const N: usize>(memory: &Memory, address: u32) -> Result<[u32; N], Fault> {
        let mut words = [0; N];
        for (index, word) in words.iter_mut().enumerate() {
            *word = read_aligned(memory, address.wrapping_add(index as u32 * 4))?;
        }
        Ok(words)
    }

    fn reference_write<const N: usize>(memory: &mut Memory, address: u32, words: [u32; N]) -> Result<(), Fault> {
        for (index, word) in words.into_iter().enumerate() {
            write_aligned(memory, address.wrapping_add(index as u32 * 4), word)?;
        }
        Ok(())
    }

    fn raw(memory: &Memory) -> Vec<(u32, Vec<u8>, Perm)> {
        memory.regions.iter().map(|r| (r.base, r.bytes.clone(), r.perm)).collect()
    }

    fn compare<const N: usize>(template: &Memory) {
        for address in [
            0,
            1,
            0x4000,
            0x4004,
            0x4008,
            0x400c,
            0x4010,
            0x401c,
            0x4020,
            0x8000,
            0xa000,
            0x10000,
            0x10ffc,
            0x11fc0,
            0x11ffc,
            0x12000,
            0xffff_ffc0,
            0xffff_fff0,
            0xffff_fffc,
            0xffff_ffff,
        ] {
            let mut batch = template.clone();
            let mut scalar = template.clone();
            scalar.block_words = false;
            let words: [u32; N] = std::array::from_fn(|index| (index as u32).wrapping_mul(0x9e37_79b9) ^ address);
            assert_eq!(
                batch.read_aligned_words::<N>(address),
                reference_read::<N>(&scalar, address),
                "read N={N} at {address:#x}"
            );
            assert_eq!(
                batch.write_aligned_words(address, words),
                reference_write(&mut scalar, address, words),
                "write N={N} at {address:#x}"
            );
            assert_eq!(raw(&batch), raw(&scalar), "partial effects N={N} at {address:#x}");
        }
    }

    #[test]
    fn block_words_match_scalar_order_permissions_alignment_boundaries_and_wrap() {
        let memory = mapped();
        assert!(memory.block_location(0x10000, 64, true).is_some());
        assert!(memory.block_location(0x10ffc, 64, true).is_some(), "crossing a page within one region is safe");
        assert!(memory.block_location(0x11ffc, 16, true).is_none());
        assert!(memory.block_location(0x4000, 16, true).is_none(), "partial indexed pages retain scalar access");
        assert!(memory.block_location(0xa000, 16, false).is_some());
        assert!(memory.block_location(0xa000, 16, true).is_none());
        assert!(memory.block_location(0x8000, 16, false).is_none());
        assert!(memory.block_location(0x8000, 16, true).is_some());
        compare::<0>(&memory);
        compare::<1>(&memory);
        compare::<2>(&memory);
        compare::<4>(&memory);
        compare::<5>(&memory);
        compare::<9>(&memory);
        compare::<16>(&memory);
        let mut reference = memory.clone();
        reference.block_words = false;
        assert!(reference.block_location(0x10000, 16, false).is_none());
        reference.block_words = true;
        reference.inline_words = false;
        assert!(reference.block_location(0x10000, 16, false).is_none());
    }

    #[test]
    fn block_stores_keep_prior_words_when_a_later_word_faults() {
        let mut memory = mapped();
        let old = memory.read_u8(0x4008).unwrap();
        assert_eq!(memory.write_aligned_words(0x4004, [0x12345678, 2, 3, 4]), Err(Fault::ReadOnly { address: 0x400a }));
        assert_eq!(memory.read_u32(0x4004).unwrap(), 0x12345678);
        assert_eq!(memory.read_u8(0x4008).unwrap(), old, "the faulting word retains its own atomicity");
    }

    #[test]
    fn block_words_preserve_deferred_provider_calls_failures_and_logical_views() {
        use std::{any::Any, sync::Mutex};
        #[derive(Debug)]
        struct Device {
            label: u8,
            fail: bool,
            bytes: [u8; 8],
            calls: Arc<Mutex<Vec<u8>>>,
        }
        impl DeferredBytes for Device {
            fn len(&self) -> usize {
                self.bytes.len()
            }
            fn bytes(&self) -> Result<&[u8], Fault> {
                self.calls.lock().unwrap().push(self.label);
                if self.fail {
                    Err(Fault::Device { address: 0x10008 + u32::from(self.label) * 8 })
                } else {
                    Ok(&self.bytes)
                }
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }
        for failure in [None, Some(0), Some(1)] {
            let make = || {
                let mut memory = mapped();
                let calls = Arc::new(Mutex::new(Vec::new()));
                for label in [0u8, 1] {
                    memory
                        .defer_device_write(
                            0x10008 + u32::from(label) * 8,
                            Arc::new(Device {
                                label,
                                fail: failure == Some(label),
                                bytes: [label + 0x61; 8],
                                calls: Arc::clone(&calls),
                            }),
                        )
                        .unwrap();
                }
                (memory, calls)
            };
            for write in [false, true] {
                let run = |batch| {
                    let (mut memory, calls) = make();
                    assert!(memory.block_location(0x10000, 36, write).is_none());
                    let result = if write {
                        if batch {
                            memory.write_aligned_words(0x10000, [0x1234; 9])
                        } else {
                            reference_write(&mut memory, 0x10000, [0x1234; 9])
                        }
                        .map(|()| [0; 9])
                    } else if batch {
                        memory.read_aligned_words::<9>(0x10000)
                    } else {
                        reference_read::<9>(&memory, 0x10000)
                    };
                    let trace = calls.lock().unwrap().clone();
                    let installed: Vec<_> = [0x10008, 0x10010].map(|address| memory.deferred_source(address, 8).is_some()).into();
                    (result, trace, raw(&memory), installed)
                };
                assert_eq!(run(true), run(false), "failure={failure:?}, write={write}");
            }
        }
        let (mut memory, _) = {
            let mut memory = mapped();
            let calls = Arc::new(Mutex::new(Vec::new()));
            memory
                .defer_device_write(
                    0x11000,
                    Arc::new(Device { label: 0, fail: false, bytes: [0x73; 8], calls: Arc::clone(&calls) }),
                )
                .unwrap();
            (memory, calls)
        };
        assert!(memory.slice(0x10000, 8192).is_some());
        assert!(memory.regions.iter().find(|r| r.base == 0x10000).unwrap().logical.get().is_some());
        assert!(memory.block_location(0x10000, 36, true).is_some());
        memory.write_aligned_words(0x10000, [0x55667788; 9]).unwrap();
        assert_eq!(&memory.slice(0x10000, 8192).unwrap()[..4], &0x55667788u32.to_le_bytes());
        assert_eq!(&memory.slice(0x10000, 8192).unwrap()[4096..4104], &[0x73; 8]);
    }
}

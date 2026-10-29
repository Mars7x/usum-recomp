//! Original out-of-line scalar word access, retained for same-binary comparison.
//! USUM_GUEST_INLINE_WORDS=0 selects this path when Memory is created.
use super::*;

impl Memory {
    #[inline(never)]
    pub(super) fn read_u32_reference(&self, address: u32) -> Result<u32, Fault> {
        self.read_bytes_reference(address).map(u32::from_le_bytes)
    }

    #[inline(never)]
    pub(super) fn write_u32_reference(&mut self, address: u32, value: u32) -> Result<(), Fault> {
        self.write_bytes_reference(address, value.to_le_bytes())
    }

    fn read_bytes_reference<const N: usize>(&self, address: u32) -> Result<[u8; N], Fault> {
        if let Some((i, o)) = self.find(address) {
            let r = &self.regions[i];
            if r.perm.read && o + N <= r.bytes.len() && !self.deferred.overlaps(address, N) {
                return Ok(r.bytes[o..o + N].try_into().expect("N bytes"));
            }
        }
        let mut out = [0u8; N];
        for (k, byte) in out.iter_mut().enumerate() {
            *byte = self.read_u8(address.wrapping_add(k as u32))?;
        }
        Ok(out)
    }

    fn write_bytes_reference<const N: usize>(&mut self, address: u32, bytes: [u8; N]) -> Result<(), Fault> {
        if !self.deferred.overlaps(address, N) {
            if let Some((i, o)) = self.find(address) {
                let r = &mut self.regions[i];
                if r.perm.write && N <= r.bytes.len() - o {
                    r.logical.take();
                    r.bytes[o..o + N].copy_from_slice(&bytes);
                    return Ok(());
                }
            }
        }
        self.write_range(address, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Device(Vec<u8>);
    impl DeferredBytes for Device {
        fn len(&self) -> usize {
            self.0.len()
        }
        fn bytes(&self) -> Result<&[u8], Fault> {
            Ok(&self.0)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[test]
    fn indexed_fast_words_preserve_device_data_and_invalidate_logical_views() {
        let mut fast = Memory { inline_words: true, use_page_index: true, ..Memory::new() };
        for i in 0..8 {
            let perm = match i {
                6 => Perm { read: false, write: true },
                7 => Perm::R,
                _ => Perm::RW,
            };
            fast.map(0x10000 + i * 0x2000, vec![0x17; 4096], perm).unwrap();
        }
        fast.defer_device_write(0x10020, Arc::new(Device(vec![0x73; 8]))).unwrap();
        let mut reference = fast.clone();
        reference.inline_words = false;
        assert!(fast.word_location(0x10000, true).is_some());
        assert!(fast.word_location(0x10020, false).is_none());
        assert!(fast.word_location(0x10ffe, false).is_none());
        assert!(fast.word_location(0x1c000, false).is_none());
        assert!(fast.word_location(0x1c000, true).is_some());
        assert!(fast.word_location(0x1e000, false).is_some());
        assert!(fast.word_location(0x1e000, true).is_none());
        assert_eq!(fast.slice(0x10000, 4096), reference.slice(0x10000, 4096));
        assert!(fast.regions[0].logical.get().is_some());
        for address in [0x10000, 0x1001e, 0x10020, 0x10024, 0x10040, 0x10ffe, 0x1c000, 0x1e000] {
            assert_eq!(fast.read_u32(address), reference.read_u32(address));
            assert_eq!(fast.write_u32(address, address ^ 0x12345678), reference.write_u32(address, address ^ 0x12345678));
            assert_eq!(fast.slice(0x10000, 4096), reference.slice(0x10000, 4096));
        }
    }

    #[test]
    fn inline_and_reference_words_match_at_boundaries_permissions_and_wraparound() {
        let mut fast = Memory { inline_words: true, ..Memory::new() };
        fast.map(0x1001, vec![0x17; 17], Perm::RW).unwrap();
        fast.map(0x1012, vec![0x93; 17], Perm::R).unwrap();
        fast.map(u32::MAX - 3, vec![0x54; 4], Perm::RW).unwrap();
        fast.map(0, vec![0x72; 4], Perm::RW).unwrap();
        let mut reference = fast.clone();
        reference.inline_words = false;
        for address in (0x0ffc..0x1028).chain([u32::MAX - 4, u32::MAX - 3, u32::MAX - 1, u32::MAX, 0, 1, 2]) {
            assert_eq!(fast.read_u32(address), reference.read_u32(address), "read {address:#x}");
            assert_eq!(
                fast.write_u32(address, address ^ 0x89abcdef),
                reference.write_u32(address, address ^ 0x89abcdef),
                "write {address:#x}"
            );
            assert_eq!(fast.read_range(0x1001, 34), reference.read_range(0x1001, 34));
            assert_eq!(fast.read_range(u32::MAX - 3, 8), reference.read_range(u32::MAX - 3, 8));
        }
    }
}

//! Immutable region indices for full guest pages. Partial pages keep the
//! original checked region search, including multiple mappings on one page.
use super::Region;
use std::sync::Arc;

pub(super) const UNMAPPED: u32 = 0;
pub(super) const PARTIAL: u32 = u32::MAX;
const SHIFT: u32 = 12;
const PAGE_SIZE: u64 = 1 << SHIFT;

#[derive(Clone)]
pub(super) struct PageIndex {
    pages: Arc<[u32]>,
}

impl std::fmt::Debug for PageIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageIndex").field("pages", &self.pages.len()).finish()
    }
}

impl PageIndex {
    pub fn new(regions: &[Region]) -> Self {
        let mut pages = vec![UNMAPPED; 1 << (32 - SHIFT)];
        for (i, region) in regions.iter().enumerate() {
            let start = u64::from(region.base);
            let end = start + region.bytes.len() as u64;
            if !start.is_multiple_of(PAGE_SIZE) {
                pages[(start >> SHIFT) as usize] = PARTIAL;
            }
            if !end.is_multiple_of(PAGE_SIZE) {
                pages[((end - 1) >> SHIFT) as usize] = PARTIAL;
            }
            let begin = start.div_ceil(PAGE_SIZE) as usize;
            let end = (end / PAGE_SIZE) as usize;
            if begin < end {
                // Each full-page region consumes at least one of 2^20 pages,
                // so its index fits unless the space has billions of tiny
                // partial mappings. Those pages can safely use the slow path.
                let entry = u32::try_from(i + 1).ok().filter(|&v| v != PARTIAL).unwrap_or(PARTIAL);
                pages[begin..end].fill(entry);
            }
        }
        Self { pages: pages.into() }
    }

    #[inline]
    pub fn lookup(&self, address: u32) -> u32 {
        self.pages[(address >> SHIFT) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn compare(fast: &Memory, reference: &Memory) {
        for address in (0..0xA100).step_by(11).chain([0xFFFF_EFFF, 0xFFFF_F000, 0xFFFF_FFFD, 0xFFFF_FFFF]) {
            assert_eq!(fast.read_u8(address), reference.read_u8(address), "byte at {address:#x}");
            assert_eq!(fast.read_u32(address), reference.read_u32(address), "word at {address:#x}");
        }
    }

    #[test]
    fn page_index_matches_reference_through_partial_pages_remap_rollback_and_clone() {
        let mut fast = Memory { use_page_index: true, ..Memory::new() };
        let mut reference = Memory { use_page_index: false, inline_words: false, ..Memory::new() };
        for m in [&mut fast, &mut reference] {
            for i in 0..8 {
                m.map(0x10000 + i * 0x2000, vec![i as u8; 0x1000], Perm::RW).unwrap();
            }
            m.map(0x1001, vec![0x17; 0x1100], Perm::RW).unwrap();
            m.map(0x2101, vec![0x93; 0x1800], Perm::R).unwrap();
            m.map(0x4000, vec![0x34; 0x2000], Perm::RW).unwrap();
            m.map(0xFFFF_F000, vec![0xAD; 0x1000], Perm::RW).unwrap();
        }
        assert!(fast.page_index.is_some());
        compare(&fast, &reference);
        let original = fast.clone();
        for m in [&mut fast, &mut reference] {
            m.remap(0x1001, 0x1000, 0x7001, Perm::R).unwrap();
            assert_eq!(m.remap(0x7001, 0x1000, 0x2101, Perm::RW), Err(Overlap));
            assert_eq!(m.write_u32(0x7002, 3), Err(Fault::ReadOnly { address: 0x7002 }));
            m.write_u32(0x4FFF, 0x12345678).unwrap();
            m.remap(0x4000, 0x1000, 0x6000, Perm::RW).unwrap();
            m.remap(0x6000, 0x1000, 0x4000, Perm::RW).unwrap();
        }
        compare(&fast, &reference);
        assert_eq!(original.read_u8(0x1001), Ok(0x17), "clone retains the old address map");
        assert_eq!(fast.read_u8(0x1001), Err(Fault::Unmapped { address: 0x1001, write: false }));
        // Mapping below all existing regions shifts every cached region index.
        fast.map(0, vec![0xEA; 0x1000], Perm::R).unwrap();
        reference.map(0, vec![0xEA; 0x1000], Perm::R).unwrap();
        compare(&fast, &reference);
    }
}

//! The guest's 32-bit address space: a few mapped regions, each with
//! permissions. Accesses are little-endian and checked. Alignment rules
//! belong to the instructions (see `ops`), not to memory.
//!
//! This is the simple, obviously correct version. A faster one (page
//! table of slices) must keep the same interface and pass the same tests.

use crate::Fault;

mod deferred;
mod index;
use deferred::DeferredRanges;
pub use deferred::{DeferredBytes, MemorySpan};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Perm {
    pub read: bool,
    pub write: bool,
}

impl Perm {
    pub const R: Perm = Perm { read: true, write: false };
    pub const RW: Perm = Perm { read: true, write: true };
}

#[derive(Debug, Clone)]
struct Region {
    base: u32,
    perm: Perm,
    bytes: Vec<u8>,
    logical: OnceLock<Result<Vec<u8>, Fault>>,
}

impl Region {
    /// The offset of `address` if it is inside, as checked arithmetic.
    fn offset(&self, address: u32) -> Option<usize> {
        let offset = address.checked_sub(self.base)? as usize;
        (offset < self.bytes.len()).then_some(offset)
    }
}

#[derive(Debug, Clone)]
pub struct Memory {
    regions: Vec<Region>,
    deferred: DeferredRanges,
    page_index: Option<index::PageIndex>,
    use_page_index: bool,
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            regions: Vec::new(),
            deferred: DeferredRanges::default(),
            page_index: None,
            use_page_index: std::env::var("USUM_GUEST_PAGE_INDEX").as_deref() != Ok("0"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Overlap;

impl Memory {
    pub fn new() -> Memory {
        Memory::default()
    }

    /// Maps `bytes` at `base`. Regions may not overlap or wrap past 4 GiB.
    pub fn map(&mut self, base: u32, bytes: Vec<u8>, perm: Perm) -> Result<(), Overlap> {
        let end = u64::from(base) + bytes.len() as u64;
        if bytes.is_empty() || end > 1 << 32 {
            return Err(Overlap);
        }
        let overlaps =
            self.regions.iter().any(|r| u64::from(r.base) < end && u64::from(base) < u64::from(r.base) + r.bytes.len() as u64);
        if overlaps {
            return Err(Overlap);
        }
        self.regions.push(Region { base, perm, bytes, logical: OnceLock::new() });
        self.regions.sort_by_key(|r| r.base);
        self.refresh_page_index();
        Ok(())
    }

    fn refresh_page_index(&mut self) {
        // Tiny synthetic address spaces are cheaper to search directly and
        // should not allocate a whole guest page table.
        self.page_index = (self.use_page_index && self.regions.len() >= 8).then(|| index::PageIndex::new(&self.regions));
    }

    fn find(&self, address: u32) -> Option<(usize, usize)> {
        if let Some(pages) = &self.page_index {
            match pages.lookup(address) {
                index::UNMAPPED => return None,
                index::PARTIAL => {}
                entry => {
                    let i = (entry - 1) as usize;
                    return Some((i, address.wrapping_sub(self.regions[i].base) as usize));
                }
            }
        }
        // regions are sorted: the candidate is the last one starting at or below
        let index = self.regions.partition_point(|r| r.base <= address).checked_sub(1)?;
        self.regions[index].offset(address).map(|offset| (index, offset))
    }

    pub fn read_u8(&self, address: u32) -> Result<u8, Fault> {
        match self.find(address) {
            Some((i, o)) if self.regions[i].perm.read => {
                if let Some(bytes) = self.deferred.read(address, 1)? {
                    return Ok(bytes[0]);
                }
                Ok(self.regions[i].bytes[o])
            }
            _ => Err(Fault::Unmapped { address, write: false }),
        }
    }

    pub fn write_u8(&mut self, address: u32, value: u8) -> Result<(), Fault> {
        self.write_bytes(address, [value])
    }

    /// Reads `N` bytes starting at `address`, byte by byte when the
    /// access straddles a region boundary.
    fn read_bytes<const N: usize>(&self, address: u32) -> Result<[u8; N], Fault> {
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

    fn write_bytes<const N: usize>(&mut self, address: u32, bytes: [u8; N]) -> Result<(), Fault> {
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

    pub fn read_u16(&self, address: u32) -> Result<u16, Fault> {
        self.read_bytes(address).map(u16::from_le_bytes)
    }

    pub fn read_u32(&self, address: u32) -> Result<u32, Fault> {
        self.read_bytes(address).map(u32::from_le_bytes)
    }

    pub fn write_u16(&mut self, address: u32, value: u16) -> Result<(), Fault> {
        self.write_bytes(address, value.to_le_bytes())
    }

    pub fn write_u32(&mut self, address: u32, value: u32) -> Result<(), Fault> {
        self.write_bytes(address, value.to_le_bytes())
    }

    /// The mapped regions, in address order: base, bytes, permission. For
    /// diagnostics and host services; guest accesses use the checked
    /// accessors.
    pub fn regions(&self) -> impl Iterator<Item = (u32, &[u8], Perm)> {
        self.try_regions().expect("deferred device read failed during memory inspection").into_iter()
    }

    /// Moves `len` bytes at `from`, which must lie in one region, to a
    /// new region at `to` with `perm`; `from` is left unmapped. This
    /// models a kernel memory alias whose source becomes inaccessible.
    pub fn remap(&mut self, from: u32, len: u32, to: u32, perm: Perm) -> Result<(), Overlap> {
        let (i, o) = self.find(from).ok_or(Overlap)?;
        let (o, len) = (o, len as usize);
        if len == 0 || o + len > self.regions[i].bytes.len() {
            return Err(Overlap);
        }
        self.materialize_all().map_err(|_| Overlap)?;
        let r = self.regions.remove(i);
        let moved = r.bytes[o..o + len].to_vec();
        let mut pieces = Vec::new();
        if o > 0 {
            pieces.push(Region { base: r.base, perm: r.perm, bytes: r.bytes[..o].to_vec(), logical: OnceLock::new() });
        }
        if o + len < r.bytes.len() {
            pieces.push(Region {
                base: r.base + (o + len) as u32,
                perm: r.perm,
                bytes: r.bytes[o + len..].to_vec(),
                logical: OnceLock::new(),
            });
        }
        let saved = self.regions.clone();
        self.regions.extend(pieces);
        self.regions.sort_by_key(|r| r.base);
        if self.map(to, moved, perm).is_err() {
            // put everything back as it was
            self.regions = saved;
            self.regions.push(r);
            self.regions.sort_by_key(|r| r.base);
            self.refresh_page_index();
            return Err(Overlap);
        }
        self.coalesce();
        Ok(())
    }

    /// Merges regions that touch and have the same permission, so a later
    /// remap can take a range that spans what earlier remaps split.
    fn coalesce(&mut self) {
        let mut merged: Vec<Region> = Vec::with_capacity(self.regions.len());
        for r in std::mem::take(&mut self.regions) {
            match merged.last_mut() {
                Some(last) if last.perm == r.perm && last.base as u64 + last.bytes.len() as u64 == u64::from(r.base) => {
                    last.logical.take();
                    last.bytes.extend_from_slice(&r.bytes);
                }
                _ => merged.push(r),
            }
        }
        self.regions = merged;
        self.refresh_page_index();
    }

    /// Copies `bytes` in (for host services such as file reads). Every
    /// byte is checked first, so a faulting copy writes nothing.
    pub fn write_range(&mut self, address: u32, bytes: &[u8]) -> Result<(), Fault> {
        self.copy_in(address, bytes, false)
    }

    /// Writes mapped memory regardless of process write permissions.
    pub fn device_write(&mut self, address: u32, bytes: &[u8]) -> Result<(), Fault> {
        self.copy_in(address, bytes, true)
    }

    fn copy_in(&mut self, address: u32, bytes: &[u8], device: bool) -> Result<(), Fault> {
        if let Some((i, o)) = self.find(address) {
            let r = &self.regions[i];
            if (device || r.perm.write) && bytes.len() <= r.bytes.len() - o {
                self.prepare_overwrite(address, bytes.len())?;
                self.regions[i].logical.take();
                self.regions[i].bytes[o..o + bytes.len()].copy_from_slice(bytes);
                return Ok(());
            }
        }
        // Validate before resolving or removing any deferred data. A failed
        // write must preserve the entire logical memory value.
        self.validate_write(address, bytes.len(), device)?;
        self.prepare_overwrite(address, bytes.len())?;
        self.copy_raw(address, bytes);
        Ok(())
    }

    fn validate_write(&self, address: u32, len: usize, device: bool) -> Result<(), Fault> {
        let mut k = 0;
        while k < len {
            let a = address.wrapping_add(k as u32);
            let (i, o) = self.find(a).ok_or(Fault::Unmapped { address: a, write: true })?;
            let r = &self.regions[i];
            if !device && !r.perm.write {
                return Err(Fault::ReadOnly { address: a });
            }
            k += (r.bytes.len() - o).min(len - k);
        }
        Ok(())
    }

    fn copy_raw(&mut self, address: u32, bytes: &[u8]) {
        let mut k = 0;
        while k < bytes.len() {
            let (i, o) = self.find(address.wrapping_add(k as u32)).expect("validated device write");
            let r = &mut self.regions[i];
            r.logical.take();
            let n = (r.bytes.len() - o).min(bytes.len() - k);
            r.bytes[o..o + n].copy_from_slice(&bytes[k..k + n]);
            k += n;
        }
    }

    /// `len` readable bytes at `address` as one slice, if they lie in one
    /// region (for devices reading large blocks: textures, buffers).
    pub fn slice(&self, address: u32, len: u32) -> Option<&[u8]> {
        let (i, o) = self.find(address)?;
        let r = &self.regions[i];
        if !r.perm.read || o + len as usize > r.bytes.len() {
            return None;
        }
        if self.deferred.overlaps(address, len as usize) {
            if let Some(bytes) = self.deferred.read(address, len as usize).ok().flatten() {
                return Some(bytes);
            }
            // A mixed RAM/device slice still lies in one mapped region. Keep
            // that API guarantee for texture aliases and DMA. This rare cache
            // is invalidated by any write to the region or its device overlays.
            let bytes = r.logical.get_or_init(|| self.logical_region(i)).as_ref().ok()?;
            return Some(&bytes[o..o + len as usize]);
        }
        Some(&r.bytes[o..o + len as usize])
    }

    /// Copies `len` bytes out (for host services such as `write`).
    pub fn read_range(&self, address: u32, len: u32) -> Result<Vec<u8>, Fault> {
        if let Some(bytes) = self.slice(address, len) {
            return Ok(bytes.to_vec());
        }
        (0..len).map(|k| self.read_u8(address.wrapping_add(k))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remapping_back_rejoins_the_region() {
        let mut m = Memory::new();
        m.map(0x1000, (0..32).collect(), Perm::RW).unwrap();
        m.remap(0x1008, 8, 0x8000, Perm::R).unwrap();
        m.remap(0x8000, 8, 0x1008, Perm::RW).unwrap();
        // one region again: a remap across the old seams works
        m.remap(0x1004, 16, 0x9000, Perm::R).unwrap();
        assert_eq!(m.read_range(0x9000, 16).unwrap(), (4..20).collect::<Vec<u8>>());
        assert_eq!(m.regions().count(), 3);
    }

    #[test]
    fn device_writes_ignore_permissions_but_not_holes() {
        let mut m = Memory::new();
        m.map(0x1000, vec![0; 4], Perm::R).unwrap();
        m.map(0x1004, vec![0; 4], Perm::R).unwrap();
        m.device_write(0x1002, &[1, 2, 3]).unwrap();
        assert_eq!(m.read_range(0x1000, 8).unwrap(), [0, 0, 1, 2, 3, 0, 0, 0]);
        assert!(m.device_write(0x1006, &[9, 9, 9]).is_err());
        assert_eq!(m.read_range(0x1006, 2).unwrap(), [0, 0], "nothing written");
    }

    #[test]
    fn remap_moves_a_range_and_leaves_a_hole() {
        let mut m = Memory::new();
        m.map(0x1000, (0..16).collect(), Perm::RW).unwrap();
        m.remap(0x1004, 8, 0x8000, Perm::R).unwrap();
        assert_eq!(m.read_range(0x8000, 8).unwrap(), [4, 5, 6, 7, 8, 9, 10, 11]);
        assert!(m.write_u8(0x8000, 0).is_err(), "new permission");
        assert!(m.read_u8(0x1004).is_err() && m.read_u8(0x100B).is_err(), "source gone");
        assert_eq!((m.read_u8(0x1003).unwrap(), m.read_u8(0x100C).unwrap()), (3, 12), "the rest stays");
        // a failed remap changes nothing
        assert_eq!(m.remap(0x1000, 4, 0x100C, Perm::R), Err(Overlap));
        assert_eq!(m.read_range(0x1000, 4).unwrap(), [0, 1, 2, 3]);
    }

    #[test]
    fn write_range_spans_regions_and_faults_atomically() {
        let mut m = Memory::new();
        m.map(0x1000, vec![0; 4], Perm::RW).unwrap();
        m.map(0x1004, vec![0; 4], Perm::RW).unwrap();
        m.write_range(0x1002, &[1, 2, 3, 4]).unwrap();
        assert_eq!(m.read_range(0x1000, 8).unwrap(), [0, 0, 1, 2, 3, 4, 0, 0]);
        assert_eq!(m.write_range(0x1006, &[9, 9, 9]), Err(Fault::Unmapped { address: 0x1008, write: true }));
        assert_eq!(m.read_range(0x1006, 2).unwrap(), [0, 0], "nothing written");
    }

    fn two_regions() -> Memory {
        let mut m = Memory::new();
        m.map(0x1000, vec![0; 0x10], Perm::RW).unwrap();
        m.map(0x1010, vec![0xAA; 0x10], Perm::R).unwrap();
        m
    }

    #[test]
    fn little_endian_and_permissions() {
        let mut m = two_regions();
        m.write_u32(0x1000, 0x1122_3344).unwrap();
        assert_eq!(m.read_u8(0x1000).unwrap(), 0x44);
        assert_eq!(m.read_u16(0x1002).unwrap(), 0x1122);
        assert_eq!(m.write_u8(0x1010, 1), Err(Fault::ReadOnly { address: 0x1010 }));
        assert_eq!(m.read_u8(0x0FFF), Err(Fault::Unmapped { address: 0x0FFF, write: false }));
        assert_eq!(m.read_u8(0x1020), Err(Fault::Unmapped { address: 0x1020, write: false }));
    }

    #[test]
    fn straddling_accesses() {
        let mut m = two_regions();
        assert_eq!(m.read_u32(0x100E).unwrap(), 0xAAAA_0000);
        // a write straddling into the read-only region changes nothing
        assert_eq!(m.write_u32(0x100E, 0xFFFF_FFFF), Err(Fault::ReadOnly { address: 0x1010 }));
        assert_eq!(m.read_u16(0x100E).unwrap(), 0);
        assert!(m.read_u32(u32::MAX - 1).is_err(), "wraps into unmapped memory");
    }

    #[test]
    fn rejects_overlap_and_wrap() {
        let mut m = two_regions();
        assert_eq!(m.map(0x100F, vec![0; 2], Perm::RW), Err(Overlap));
        assert_eq!(m.map(0xFFFF_FFF0, vec![0; 0x20], Perm::RW), Err(Overlap));
        assert!(m.map(0xFFFF_FFF0, vec![0; 0x10], Perm::RW).is_ok());
        assert_eq!(m.read_u8(0xFFFF_FFFF).unwrap(), 0);
    }
}

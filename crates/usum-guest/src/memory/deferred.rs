//! Immutable device results overlaid on RAM until the guest needs their bytes.
//! Providers own their snapshots: cloning Memory never observes a newer GPU draw.
use super::*;
use std::any::Any;
use std::fmt::Debug;

/// A frozen device write, optionally materialized on first read. Implementations
/// must return the same bytes for their entire lifetime, including across clones.
/// A OnceLock-backed readback ticket can satisfy this without unsafe RAM access.
pub trait DeferredBytes: Debug + Send + Sync + 'static {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn bytes(&self) -> Result<&[u8], Fault>;
    fn as_any(&self) -> &dyn Any;
}

#[derive(Debug, Clone)]
struct Range {
    address: u32,
    source: Arc<dyn DeferredBytes>,
    len: usize,
}

impl Range {
    fn end(&self) -> u64 {
        u64::from(self.address) + self.len as u64
    }
    fn bytes(&self) -> Result<&[u8], Fault> {
        let bytes = self.source.bytes()?;
        if bytes.len() != self.len {
            return Err(Fault::Device { address: self.address });
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct DeferredRanges {
    ranges: Vec<Range>,
    // Empty has low == high. Most guest accesses never enter device memory.
    low: u64,
    high: u64,
}

// Guest scalar accesses may wrap at 4 GiB, even though individual mappings and
// deferred ranges cannot. Express such a write as at most two linear intervals.
fn segments(address: u32, len: usize) -> [(u64, u64); 2] {
    let start = u64::from(address);
    let end = start + len as u64;
    [(start, end.min(1 << 32)), (0, end.saturating_sub(1 << 32))]
}

impl DeferredRanges {
    pub(super) fn overlaps(&self, address: u32, len: usize) -> bool {
        len != 0 && segments(address, len).into_iter().any(|(a, b)| a < self.high && self.low < b && a < b)
    }

    pub(super) fn read(&self, address: u32, len: usize) -> Result<Option<&[u8]>, Fault> {
        if !self.overlaps(address, len) {
            return Ok(None);
        }
        let i = self.ranges.partition_point(|r| r.address <= address);
        if let Some(r) = i.checked_sub(1).map(|i| &self.ranges[i]) {
            let end = u64::from(address) + len as u64;
            if end <= r.end() {
                let o = (address - r.address) as usize;
                return Ok(Some(&r.bytes()?[o..o + len]));
            }
        }
        Ok(None)
    }

    fn refresh_bounds(&mut self) {
        self.low = self.ranges.first().map_or(0, |r| u64::from(r.address));
        self.high = self.ranges.last().map_or(0, Range::end);
    }
}

/// One borrowed logical memory span: address, bytes, process permissions.
pub type MemorySpan<'a> = (u32, &'a [u8], Perm);

impl Memory {
    /// Replace a mapped, contiguous device range with an immutable result. The
    /// provider is not evaluated here. Full overwrites discard old tickets;
    /// partial overwrites first preserve their untouched bytes in RAM.
    pub fn defer_device_write(&mut self, address: u32, source: Arc<dyn DeferredBytes>) -> Result<(), Fault> {
        let len = source.len();
        let (i, o) = self.find(address).ok_or(Fault::Unmapped { address, write: true })?;
        if len == 0 || len > self.regions[i].bytes.len() - o {
            return Err(Fault::Device { address });
        }
        self.prepare_overwrite(address, len)?;
        let i = self.deferred.ranges.partition_point(|r| r.address < address);
        self.deferred.ranges.insert(i, Range { address, source, len });
        self.deferred.refresh_bounds();
        Ok(())
    }

    /// Exact-range identity for a device that can reuse its resident result.
    /// This never triggers a readback. Any overlapping CPU write removes it.
    pub fn deferred_source(&self, address: u32, len: usize) -> Option<&dyn DeferredBytes> {
        if !self.deferred.overlaps(address, len) {
            return None;
        }
        let i = self.deferred.ranges.binary_search_by_key(&address, |r| r.address).ok()?;
        let r = &self.deferred.ranges[i];
        (r.len == len).then_some(r.source.as_ref())
    }

    pub(super) fn prepare_overwrite(&mut self, address: u32, len: usize) -> Result<(), Fault> {
        if !self.deferred.overlaps(address, len) {
            return Ok(());
        }
        let spans = segments(address, len);
        let overlaps = |r: &Range| spans.iter().any(|&(a, b)| a < r.end() && u64::from(r.address) < b && a < b);
        let covers = |r: &Range| spans.iter().any(|&(a, b)| a <= u64::from(r.address) && r.end() <= b);
        // Resolve every partial range before dropping anything. If any provider
        // fails, all old logical values remain installed (including full writes).
        let partial: Vec<_> = self.deferred.ranges.iter().filter(|r| overlaps(r) && !covers(r)).cloned().collect();
        for r in &partial {
            r.bytes()?;
        }
        for r in &partial {
            self.copy_raw(r.address, r.bytes()?);
        }
        self.deferred.ranges.retain(|r| !overlaps(r));
        self.deferred.refresh_bounds();
        Ok(())
    }

    pub(super) fn materialize_all(&mut self) -> Result<(), Fault> {
        let ranges = self.deferred.ranges.clone();
        for r in &ranges {
            r.bytes()?;
        }
        for r in &ranges {
            self.copy_raw(r.address, r.bytes()?);
        }
        self.deferred = DeferredRanges::default();
        Ok(())
    }

    /// Logical mapped contents for diagnostics and snapshots. A device failure
    /// is returned rather than exposing the stale RAM beneath a pending write.
    /// Device overlays may split one mapping into several adjacent slices.
    pub fn try_regions(&self) -> Result<Vec<MemorySpan<'_>>, Fault> {
        let mut out = Vec::new();
        for r in &self.regions {
            let mut offset = 0;
            for pending in
                self.deferred.ranges.iter().filter(|p| p.address >= r.base && p.end() <= u64::from(r.base) + r.bytes.len() as u64)
            {
                let p = (pending.address - r.base) as usize;
                if p > offset {
                    out.push((r.base + offset as u32, &r.bytes[offset..p], r.perm));
                }
                out.push((pending.address, pending.bytes()?, r.perm));
                offset = p + pending.len;
            }
            if offset < r.bytes.len() {
                out.push((r.base + offset as u32, &r.bytes[offset..], r.perm));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    };

    #[derive(Debug)]
    struct Ticket {
        len: usize,
        value: u8,
        calls: AtomicUsize,
        cache: OnceLock<Vec<u8>>,
        fail: bool,
        malformed: bool,
    }
    impl DeferredBytes for Ticket {
        fn len(&self) -> usize {
            self.len
        }
        fn bytes(&self) -> Result<&[u8], Fault> {
            if self.fail {
                return Err(Fault::Device { address: 0x1004 });
            }
            Ok(self.cache.get_or_init(|| {
                self.calls.fetch_add(1, Ordering::Relaxed);
                vec![self.value; self.len - usize::from(self.malformed)]
            }))
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    fn ticket(len: usize, value: u8) -> Arc<Ticket> {
        Arc::new(Ticket { len, value, calls: AtomicUsize::new(0), cache: OnceLock::new(), fail: false, malformed: false })
    }
    fn memory() -> Memory {
        let mut m = Memory::new();
        m.map(0x1000, (0..16).collect(), Perm::RW).unwrap();
        m
    }
    #[test]
    fn lazy_reads_slices_and_clones_see_frozen_results() {
        let mut m = memory();
        let t = ticket(4, 99);
        m.defer_device_write(0x1004, t.clone()).unwrap();
        let snapshot = m.clone();
        assert_eq!(t.calls.load(Ordering::Relaxed), 0);
        assert_eq!(m.read_u8(0x1000).unwrap(), 0);
        assert_eq!(m.slice(0x1004, 4).unwrap(), [99; 4]);
        assert_eq!(m.read_u32(0x1002).unwrap(), 0x6363_0302);
        assert_eq!(snapshot.read_u32(0x1004).unwrap(), 0x6363_6363);
        assert_eq!(t.calls.load(Ordering::Relaxed), 1);
        m.write_u8(0x1005, 7).unwrap();
        assert_eq!(m.read_range(0x1004, 4).unwrap(), [99, 7, 99, 99]);
        assert_eq!(snapshot.read_range(0x1004, 4).unwrap(), [99; 4]);
        assert!(m.deferred_source(0x1004, 4).is_none());
    }
    #[test]
    fn full_overwrites_discard_without_readback() {
        let mut m = memory();
        let t = ticket(4, 99);
        m.defer_device_write(0x1004, t.clone()).unwrap();
        m.write_u32(0x1004, 7).unwrap();
        assert_eq!(t.calls.load(Ordering::Relaxed), 0);
        assert_eq!(m.read_u32(0x1004).unwrap(), 7);
        m.defer_device_write(0x1004, t.clone()).unwrap();
        m.defer_device_write(0x1002, ticket(8, 42)).unwrap();
        assert_eq!(t.calls.load(Ordering::Relaxed), 0);
        assert_eq!(m.read_range(0x1000, 12).unwrap(), [0, 1, 42, 42, 42, 42, 42, 42, 42, 42, 10, 11]);
    }
    #[test]
    fn failed_cpu_and_device_writes_preserve_pending_data() {
        let mut m = memory();
        m.map(0x1010, vec![0; 4], Perm::R).unwrap();
        let t = ticket(4, 99);
        m.defer_device_write(0x100c, t.clone()).unwrap();
        assert_eq!(m.write_range(0x100c, &[5; 8]), Err(Fault::ReadOnly { address: 0x1010 }));
        assert!(m.device_write(0x100c, &[5; 9]).is_err());
        assert_eq!(t.calls.load(Ordering::Relaxed), 0);
        assert!(m.deferred_source(0x100c, 4).is_some());
        assert_eq!(m.read_range(0x100c, 4).unwrap(), [99; 4]);
        m.defer_device_write(0x1010, ticket(4, 88)).unwrap();
        assert!(m.write_u8(0x1010, 0).is_err());
        m.device_write(0x1011, &[7]).unwrap();
        assert_eq!(m.read_range(0x1010, 4).unwrap(), [88, 7, 88, 88]);
    }
    #[test]
    fn failed_partial_resolution_keeps_even_fully_overwritten_tickets() {
        let mut m = memory();
        let first = ticket(4, 99);
        let bad = Arc::new(Ticket {
            fail: true,
            ..Ticket { len: 4, value: 0, calls: AtomicUsize::new(0), cache: OnceLock::new(), fail: false, malformed: false }
        });
        m.defer_device_write(0x1000, first.clone()).unwrap();
        m.defer_device_write(0x1004, bad).unwrap();
        assert!(m.write_range(0x1000, &[7; 6]).is_err());
        assert!(m.deferred_source(0x1000, 4).is_some());
        assert!(m.deferred_source(0x1004, 4).is_some());
        assert_eq!(first.calls.load(Ordering::Relaxed), 0);
        assert_eq!(m.read_u32(0x1000).unwrap(), 0x6363_6363);
    }
    #[test]
    fn regions_and_remap_include_device_bytes() {
        let mut m = memory();
        m.defer_device_write(0x1004, ticket(4, 99)).unwrap();
        let regions = m.try_regions().unwrap();
        assert_eq!(regions.iter().map(|r| r.0).collect::<Vec<_>>(), [0x1000, 0x1004, 0x1008]);
        assert_eq!(regions.iter().flat_map(|r| r.1.iter().copied()).collect::<Vec<_>>(), m.read_range(0x1000, 16).unwrap());
        m.remap(0x1002, 8, 0x8000, Perm::R).unwrap();
        assert_eq!(m.read_range(0x8000, 8).unwrap(), [2, 3, 99, 99, 99, 99, 8, 9]);
    }
    #[test]
    fn malformed_results_fault_instead_of_exposing_stale_ram() {
        let mut m = memory();
        let mut t = ticket(4, 99);
        Arc::get_mut(&mut t).unwrap().malformed = true;
        m.defer_device_write(0x1004, t).unwrap();
        assert_eq!(m.read_u8(0x1004), Err(Fault::Device { address: 0x1004 }));
        assert!(m.slice(0x1004, 4).is_none());
        assert!(m.read_range(0x1004, 4).is_err());
        assert!(m.try_regions().is_err());
    }
    #[test]
    fn mixed_device_cpu_operations_match_eager_memory() {
        let mut lazy = memory();
        let mut eager = memory();
        let mut random = 0x851a_923bu32;
        for step in 0..2000 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let offset = (random >> 24) as usize % 16;
            let len = ((random >> 16) as usize % (16 - offset)) + 1;
            let address = 0x1000 + offset as u32;
            let value = (random >> 8) as u8;
            if step % 3 == 0 {
                lazy.defer_device_write(address, ticket(len, value)).unwrap();
                eager.device_write(address, &vec![value; len]).unwrap();
            } else {
                lazy.write_range(address, &vec![value; len]).unwrap();
                eager.write_range(address, &vec![value; len]).unwrap();
            }
            assert_eq!(lazy.read_range(0x1000, 16), eager.read_range(0x1000, 16), "step {step}");
            for offset in 0..13 {
                assert_eq!(lazy.read_u32(0x1000 + offset), eager.read_u32(0x1000 + offset));
            }
            if step % 29 == 0 {
                let mut clone = lazy.clone();
                clone.write_u32(0x1004, 0).unwrap();
                assert_eq!(lazy.read_range(0x1000, 16), eager.read_range(0x1000, 16));
            }
        }
    }

    #[test]
    fn wrapped_writes_preserve_untouched_pending_bytes() {
        let mut m = Memory::new();
        m.map(u32::MAX - 3, vec![0; 4], Perm::RW).unwrap();
        m.map(0, vec![0; 4], Perm::RW).unwrap();
        m.defer_device_write(u32::MAX - 3, ticket(4, 99)).unwrap();
        m.defer_device_write(0, ticket(4, 88)).unwrap();
        m.write_u32(u32::MAX - 1, 0x0403_0201).unwrap();
        assert_eq!(m.read_range(u32::MAX - 3, 8).unwrap(), [99, 99, 1, 2, 3, 4, 88, 88]);
    }
}

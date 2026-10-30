//! Reuse decoded inputs only after proving the complete source spans unchanged.
//! Addresses alone never establish identity; permissions and bytes are rechecked.
use super::{input::InputPlan, Layout, Memory, Vec4};
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

pub(in crate::pica) type Inputs = Arc<Vec<[Vec4; 16]>>;
const MAX_ENTRIES: usize = 512;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_SOURCE: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

#[derive(Debug)]
struct Entry {
    layout: Layout,
    initial: [[u32; 4]; 16],
    destinations: Vec<u8>,
    indices: Vec<u32>,
    source: Vec<(u32, Vec<u8>)>,
    inputs: Inputs,
    bytes: usize,
    age: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pica::{draw::Loader, Pica};
    use usum_guest::Perm;

    fn layout() -> Layout {
        Layout { loaders: vec![Loader { start: 0x1000, stride: 16, span: 16, fields: vec![(0, 3, 4, 0)] }], total: 2 }
    }
    fn decoded(plan: &InputPlan<'_>, memory: &Memory, indices: &[u32]) -> Inputs {
        Arc::new(indices.iter().map(|&i| plan.load(memory, i).unwrap()).collect())
    }

    #[test]
    fn exact_bits_aliases_source_and_index_order_establish_identity() {
        let mut memory = Memory::new();
        memory.map(0x1000, vec![0; 64], Perm::RW).unwrap();
        memory.write_u32(0x1000, 0x7fa1_2345).unwrap();
        let mut pica = Pica::default();
        pica.fixed[1] = [-0.0, f32::from_bits(0x7fc1_2345), 3.0, 4.0];
        let layout = layout();
        let plan = InputPlan::new(&pica, &layout);
        let ids = [0, 2, 1];
        let inputs = decoded(&plan, &memory, &ids);
        let mut cache = Cache::default();
        cache.insert(&plan, &memory, &ids, &inputs);
        assert!(Arc::ptr_eq(&cache.get(&plan, &memory, &ids).unwrap(), &inputs));
        assert!(cache.get(&plan, &memory, &[0, 1, 2]).is_none());
        pica.fixed[1][0] = 0.0;
        assert!(cache.get(&InputPlan::new(&pica, &layout), &memory, &ids).is_none());
        pica.fixed[1][0] = -0.0;
        pica.vs.permutation[1] = 0;
        assert!(cache.get(&InputPlan::new(&pica, &layout), &memory, &ids).is_none());
        memory.write_u8(0x1010, 1).unwrap();
        assert!(cache.get(&plan, &memory, &ids).is_none());
        assert!(cache.clone().entries.is_empty());
    }

    #[test]
    fn unreadable_or_remapped_discarded_attributes_cannot_hit() {
        let pica = Pica::default();
        let mut layout = layout();
        layout.total = 0; // Every field is discarded but must still be read.
        let plan = InputPlan::new(&pica, &layout);
        let mut memory = Memory::new();
        memory.map(0x1000, vec![0; 64], Perm::RW).unwrap();
        let inputs = decoded(&plan, &memory, &[0, 1]);
        let mut cache = Cache::default();
        cache.insert(&plan, &memory, &[0, 1], &inputs);
        let bytes = memory.unmap_region(0x1000, 64).unwrap();
        assert!(cache.get(&plan, &memory, &[0, 1]).is_none());
        assert!(plan.load(&memory, 0).unwrap_err().contains("0x00001000"));
        memory.map(0x1000, bytes.clone(), Perm { read: false, write: true }).unwrap();
        assert!(cache.get(&plan, &memory, &[0, 1]).is_none());
        assert!(plan.load(&memory, 0).is_err());
        memory.unmap_region(0x1000, 64).unwrap();
        memory.map(0x1000, bytes, Perm::R).unwrap();
        assert!(cache.get(&plan, &memory, &[0, 1]).is_some());
    }

    #[test]
    fn wrapping_and_gapped_spans_bypass_without_changing_decoder_errors() {
        let pica = Pica::default();
        let mut memory = Memory::new();
        memory.map(0x1000, vec![0; 16], Perm::RW).unwrap();
        memory.map(0x1020, vec![1; 16], Perm::RW).unwrap();
        let layout = layout();
        let plan = InputPlan::new(&pica, &layout);
        let inputs = decoded(&plan, &memory, &[0, 2]);
        let mut cache = Cache::default();
        cache.insert(&plan, &memory, &[0, 2], &inputs);
        assert!(cache.entries.is_empty());
        let mut wrapping = layout.clone();
        wrapping.loaders[0].stride = 0x8000_0000;
        let plan = InputPlan::new(&pica, &wrapping);
        let inputs = decoded(&plan, &memory, &[0, 2]);
        cache.insert(&plan, &memory, &[0, 2], &inputs);
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn deferred_replacement_invalidates_inputs_and_preserves_first_decoder_fault() {
        use std::any::Any;
        use usum_guest::{memory::DeferredBytes, Fault};
        #[derive(Debug)]
        struct Provider {
            fail: bool,
            bytes: [u8; 16],
        }
        impl DeferredBytes for Provider {
            fn len(&self) -> usize {
                self.bytes.len()
            }
            fn bytes(&self) -> Result<&[u8], Fault> {
                if self.fail {
                    Err(Fault::Device { address: 0x1000 })
                } else {
                    Ok(&self.bytes)
                }
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }
        let pica = Pica::default();
        let layout = layout();
        let plan = InputPlan::new(&pica, &layout);
        let mut memory = Memory::new();
        memory.map(0x1000, vec![0; 32], Perm::RW).unwrap();
        let inputs = decoded(&plan, &memory, &[0, 1]);
        let mut cache = Cache::default();
        cache.insert(&plan, &memory, &[0, 1], &inputs);
        memory.defer_device_write(0x1000, Arc::new(Provider { fail: false, bytes: [1; 16] })).unwrap();
        assert!(cache.get(&plan, &memory, &[0, 1]).is_none());
        let updated = decoded(&plan, &memory, &[0, 1]);
        cache.insert(&plan, &memory, &[0, 1], &updated);
        assert!(cache.get(&plan, &memory, &[0, 1]).is_some());
        memory.defer_device_write(0x1000, Arc::new(Provider { fail: true, bytes: [0; 16] })).unwrap();
        let expected = plan.load(&memory, 0).unwrap_err();
        assert!(cache.get(&plan, &memory, &[0, 1]).is_none());
        assert_eq!(plan.load(&memory, 0).unwrap_err(), expected);
        cache.insert(&plan, &memory, &[0, 1], &updated);
        assert!(cache.get(&plan, &memory, &[0, 1]).is_none());
    }

    #[test]
    fn retained_payload_and_entry_limits_evict_and_oversized_outputs_bypass() {
        let mut pica = Pica::default();
        let mut layout = layout();
        layout.loaders[0].stride = 0;
        let mut memory = Memory::new();
        memory.map(0x1000, vec![0; 16], Perm::R).unwrap();
        let ids: Vec<u32> = (0..8192).collect();
        let mut cache = Cache::default();
        for i in 0..20 {
            pica.fixed[1][0] = i as f32;
            let plan = InputPlan::new(&pica, &layout);
            let inputs = decoded(&plan, &memory, &ids);
            cache.insert(&plan, &memory, &ids, &inputs);
            assert!(cache.bytes <= MAX_BYTES);
        }
        assert!(cache.entries.len() < 20);
        for i in 0..MAX_ENTRIES + 10 {
            pica.fixed[1][0] = i as f32;
            let plan = InputPlan::new(&pica, &layout);
            cache.insert(&plan, &memory, &[0], &decoded(&plan, &memory, &[0]));
            assert!(cache.entries.len() <= MAX_ENTRIES && cache.bytes <= MAX_BYTES);
        }
        let plan = InputPlan::new(&pica, &layout);
        let too_large = Arc::new(vec![[[0.0; 4]; 16]; MAX_OUTPUT / 256 + 1]);
        let before = cache.stats();
        cache.insert(&plan, &memory, &[u32::MAX], &too_large);
        assert_eq!(before, cache.stats());
    }
}

#[derive(Debug, Default)]
pub(in crate::pica) struct Cache {
    entries: HashMap<u64, Entry>,
    bytes: usize,
    age: u64,
    hits: u64,
    misses: u64,
}

// A Pica snapshot starts cold instead of duplicating retained derived payloads.
impl Clone for Cache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

fn key(plan: &InputPlan<'_>, indices: &[u32]) -> u64 {
    let mut hash = DefaultHasher::new();
    plan.layout.hash(&mut hash);
    plan.initial.map(|v| v.map(f32::to_bits)).hash(&mut hash);
    plan.destinations.hash(&mut hash);
    indices.hash(&mut hash);
    hash.finish()
}

impl Cache {
    pub(in crate::pica) fn stats(&self) -> [u64; 4] {
        [self.hits, self.misses, self.entries.len() as u64, self.bytes as u64]
    }

    pub(super) fn get(&mut self, plan: &InputPlan<'_>, memory: &Memory, indices: &[u32]) -> Option<Inputs> {
        self.misses += 1;
        let entry = self.entries.get_mut(&key(plan, indices))?;
        if entry.layout != *plan.layout
            || entry.initial != plan.initial.map(|v| v.map(f32::to_bits))
            || entry.destinations != plan.destinations
            || entry.indices != indices
            || !entry.source.iter().all(|(address, bytes)| memory.slice(*address, bytes.len() as u32) == Some(bytes.as_slice()))
        {
            return None;
        }
        self.misses -= 1;
        self.hits += 1;
        self.age = self.age.wrapping_add(1);
        entry.age = self.age;
        Some(Arc::clone(&entry.inputs))
    }

    // Called only after the original decoder has successfully read every input.
    // A failed span proof bypasses caching and never replaces a decoder error.
    pub(super) fn insert(&mut self, plan: &InputPlan<'_>, memory: &Memory, indices: &[u32], inputs: &Inputs) {
        let output = inputs.capacity() * std::mem::size_of::<[Vec4; 16]>();
        if output > MAX_OUTPUT {
            return;
        }
        let Some(low) = indices.iter().copied().min() else { return };
        let high = indices.iter().copied().max().unwrap();
        let mut source = Vec::new();
        let mut source_bytes = 0usize;
        for loader in &plan.layout.loaders {
            if loader.span == 0 {
                continue;
            }
            let Some(address) = low.checked_mul(loader.stride).and_then(|n| loader.start.checked_add(n)) else { return };
            let Some(len) = (high - low).checked_mul(loader.stride).and_then(|n| n.checked_add(loader.span)) else { return };
            if u64::from(address) + u64::from(len) > 1 << 32 {
                return;
            }
            source_bytes += len as usize;
            if source_bytes > MAX_SOURCE {
                return;
            }
            let Some(bytes) = memory.slice(address, len) else { return };
            source.push((address, bytes.to_vec()));
        }
        let layout = plan.layout.clone();
        let destinations = plan.destinations.clone();
        let indices = indices.to_vec();
        let bytes = output
            + source.iter().map(|(_, b)| b.capacity()).sum::<usize>()
            + source.capacity() * std::mem::size_of::<(u32, Vec<u8>)>()
            + layout.loaders.capacity() * std::mem::size_of::<super::Loader>()
            + layout.loaders.iter().map(|l| l.fields.capacity() * std::mem::size_of::<(usize, u32, u32, u32)>()).sum::<usize>()
            + destinations.capacity()
            + indices.capacity() * std::mem::size_of::<u32>();
        if bytes > MAX_BYTES {
            return;
        }
        let hash = key(plan, &indices);
        if let Some(old) = self.entries.remove(&hash) {
            self.bytes -= old.bytes;
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes + bytes > MAX_BYTES {
            let oldest = *self.entries.iter().min_by_key(|(_, e)| e.age).unwrap().0;
            self.bytes -= self.entries.remove(&oldest).unwrap().bytes;
        }
        self.age = self.age.wrapping_add(1);
        self.bytes += bytes;
        self.entries.insert(
            hash,
            Entry {
                layout,
                initial: plan.initial.map(|v| v.map(f32::to_bits)),
                destinations,
                indices,
                source,
                inputs: Arc::clone(inputs),
                bytes,
                age: self.age,
            },
        );
    }
}

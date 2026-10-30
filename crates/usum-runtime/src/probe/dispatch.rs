//! Cache host dispatch lookups, preserving module identity checks on every hit.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

use usum_guest::{lookup, Code};

use super::{module_code, Kernel, Modules};

// Keys are verified guest code addresses, not attacker-supplied strings. Mix
// both ends of the address so aligned ARM/Thumb entries use the whole table.
#[derive(Default)]
struct AddressHasher(u64);

impl Hasher for AddressHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write_u32(&mut self, value: u32) {
        let mut value = u64::from(value);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        self.0 = value ^ (value >> 31);
    }
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
}

struct Owner {
    revision: Arc<()>,
    index: usize,
    base: u32,
    size: u32,
    name: String,
    sha256: String,
}

#[derive(Clone)]
struct CachedCode {
    code: Code,
    delta: Option<u32>,
    owner: Option<Arc<Owner>>,
}

impl CachedCode {
    fn valid(&self, kernel: &Kernel, revisions: bool) -> bool {
        self.owner.as_ref().is_none_or(|owner| {
            if revisions {
                return Arc::ptr_eq(&owner.revision, kernel.modules.revision());
            }
            kernel.modules.get(owner.index).is_some_and(|m| {
                m.base == owner.base && m.cro.file_size == owner.size && m.cro.name == owner.name && m.sha256 == owner.sha256
            })
        })
    }
}

pub(super) struct CodeCache {
    enabled: bool,
    entries: HashMap<u32, CachedCode, BuildHasherDefault<AddressHasher>>,
    front_enabled: bool,
    revision_enabled: bool,
    front: [Option<FrontSlot>; 256],
    front_hits: u64,
    hash_hits: u64,
    misses: u64,
    log_stats: bool,
}

struct FrontSlot {
    key: u32,
    code: CachedCode,
}

fn front_index(key: u32) -> usize {
    // Mix aligned instruction addresses with shifts only. Tags retain the
    // complete ARM/Thumb key; collisions always use the checked backing cache.
    ((key ^ (key >> 7) ^ (key >> 15)) & 255) as usize
}

impl Drop for CodeCache {
    fn drop(&mut self) {
        if self.log_stats {
            eprintln!("dispatch cache: {} front hits, {} hash hits, {} misses", self.front_hits, self.hash_hits, self.misses);
        }
    }
}

impl CodeCache {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            entries: HashMap::default(),
            front_enabled: enabled && std::env::var("USUM_DISPATCH_FRONT_CACHE").as_deref() != Ok("0"),
            revision_enabled: std::env::var("USUM_DISPATCH_MODULE_REVISION").as_deref() != Ok("0"),
            front: std::array::from_fn(|_| None),
            front_hits: 0,
            hash_hits: 0,
            misses: 0,
            log_stats: std::env::var("USUM_DISPATCH_STATS").as_deref() == Ok("1"),
        }
    }

    pub fn resolve(
        &mut self,
        kernel: &Kernel,
        modules: &Modules,
        entries: &[(u32, Code)],
        key: u32,
    ) -> Option<(Code, Option<u32>)> {
        let index = front_index(key);
        if self.front_enabled {
            if let Some(slot) = &self.front[index] {
                if slot.key == key && slot.code.valid(kernel, self.revision_enabled) {
                    self.front_hits += 1;
                    return Some((slot.code.code, slot.code.delta));
                }
            }
        }
        if self.enabled {
            if let Some(cached) = self.entries.get(&key).filter(|c| c.valid(kernel, self.revision_enabled)) {
                self.hash_hits += 1;
                if self.front_enabled {
                    self.front[index] = Some(FrontSlot { key, code: cached.clone() });
                }
                return Some((cached.code, cached.delta));
            }
        }
        self.misses += 1;
        let (code, delta) = match lookup(entries, key) {
            Some(code) => (code, None),
            None => {
                let (code, delta) = module_code(kernel, modules, key)?;
                (code, Some(delta))
            }
        };
        if self.enabled {
            let owner = if delta.is_some() {
                let address = key & !1;
                let (index, m) = kernel
                    .modules
                    .iter()
                    .enumerate()
                    .find(|(_, m)| m.base != 0 && address.wrapping_sub(m.base) < m.cro.file_size)?;
                Some(Arc::new(Owner {
                    revision: Arc::clone(kernel.modules.revision()),
                    index,
                    base: m.base,
                    size: m.cro.file_size,
                    name: m.cro.name.clone(),
                    sha256: m.sha256.clone(),
                }))
            } else {
                None
            };
            let cached = CachedCode { code, delta, owner };
            if self.front_enabled {
                self.front[index] = Some(FrontSlot { key, code: cached.clone() });
            }
            self.entries.insert(key, cached);
        }
        Some((code, delta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{boot::Process, ro::Loaded};
    use usum_guest::{Cpu, Exit, Fault, Memory};
    use usum_import::cro::Cro;

    fn one(cpu: &mut Cpu, _: &mut Memory) -> Result<Exit, Fault> {
        cpu.r[0] = 1;
        Ok(Exit::Jump)
    }

    fn two(cpu: &mut Cpu, _: &mut Memory) -> Result<Exit, Fault> {
        cpu.r[0] = 2;
        Ok(Exit::Jump)
    }

    fn loaded(name: &str, sha256: &str, base: u32) -> Loaded {
        Loaded {
            cro: Cro {
                name: name.into(),
                is_crs: false,
                file_size: 0x1000,
                bss_size: 0,
                segments: vec![],
                on_load: None,
                on_exit: None,
                on_unresolved: None,
                named_exports: vec![],
                indexed_exports: vec![],
                imports: vec![],
                internal: vec![],
                import_table: 0,
            },
            segments: vec![],
            base,
            unresolved: vec![],
            image: base,
            sha256: sha256.into(),
        }
    }

    #[test]
    fn front_collisions_fall_back_without_confusing_distinct_code_keys() {
        let kernel = Kernel::default();
        let first = 0x1000;
        let collision = (0x2000..0x20000).step_by(4).find(|&key| front_index(key) == front_index(first)).unwrap();
        let entries: &[(u32, Code)] = &[(first, one), (collision, two)];
        let modules = Modules { functions: &[], entries: &[], translated: &[] };
        let mut cache = CodeCache::new(true);
        cache.front_enabled = true;
        let mut cpu = Cpu::default();
        let mut memory = Memory::new();
        for _ in 0..8 {
            for (key, expected) in [(first, 1), (collision, 2)] {
                for _ in 0..2 {
                    let (code, delta) = cache.resolve(&kernel, &modules, entries, key).unwrap();
                    assert_eq!(delta, None);
                    code(&mut cpu, &mut memory).unwrap();
                    assert_eq!(cpu.r[0], expected);
                }
            }
        }
        assert!(cache.front_hits > 0 && cache.hash_hits > 0, "exercise both hit paths");
        assert_eq!(cache.misses, 2, "colliding entries remain in the backing cache");
        assert_eq!(cache.entries.len(), 2);
    }

    #[test]
    fn cached_dispatch_tracks_module_unload_replacement_relocation_and_instruction_state() {
        let mut kernel = Kernel::new(
            Process {
                linear_base: 0x3000_0000,
                app_region: 0x0500_0000,
                main_priority: 0x30,
                executable_end: 0x0070_0000,
                save_data_size: 0,
            },
            vec![],
            vec![],
        );
        static ENTRIES: &[(u32, Code)] = &[(0x1000, one), (0x1001, two)];
        static MODULE_ENTRIES: &[(u32, Code)] = &[(0x4000_0200, one), (0x5000_0200, two)];
        let modules = Modules {
            functions: &[],
            entries: MODULE_ENTRIES,
            translated: &[("first", "aaa", 0x4000_0000), ("second", "bbb", 0x5000_0000)],
        };
        let mut cpu = Cpu::default();
        let mut mem = Memory::new();
        // Both modes must agree through the same lifecycle, including cached hits.
        for (enabled, front, revisions) in
            [(false, false, false), (true, false, false), (true, true, false), (true, false, true), (true, true, true)]
        {
            let mut cache = CodeCache::new(enabled);
            cache.front_enabled = front;
            cache.revision_enabled = revisions;
            kernel.modules = vec![loaded("first", "aaa", 0x8000)].into();
            for _ in 0..2 {
                for (key, expected) in [(0x1000, 1), (0x1001, 2)] {
                    let (code, delta) = cache.resolve(&kernel, &modules, ENTRIES, key).unwrap();
                    assert_eq!(delta, None);
                    code(&mut cpu, &mut mem).unwrap();
                    assert_eq!(cpu.r[0], expected);
                }
                let (code, delta) = cache.resolve(&kernel, &modules, ENTRIES, 0x8200).unwrap();
                assert_eq!(delta, Some(0x8000u32.wrapping_sub(0x4000_0000)));
                code(&mut cpu, &mut mem).unwrap();
                assert_eq!(cpu.r[0], 1);
            }
            kernel.modules[0].base = 0;
            assert!(cache.resolve(&kernel, &modules, ENTRIES, 0x8200).is_none());
            kernel.modules[0] = loaded("second", "bbb", 0x8000);
            let (code, delta) = cache.resolve(&kernel, &modules, ENTRIES, 0x8200).unwrap();
            assert_eq!(delta, Some(0x8000u32.wrapping_sub(0x5000_0000)));
            code(&mut cpu, &mut mem).unwrap();
            assert_eq!(cpu.r[0], 2);
            kernel.modules[0].base = 0xa000;
            assert!(cache.resolve(&kernel, &modules, ENTRIES, 0x8200).is_none());
            assert!(cache.resolve(&kernel, &modules, ENTRIES, 0xa200).is_some());
            kernel.modules[0].sha256 = "untranslated".into();
            assert!(cache.resolve(&kernel, &modules, ENTRIES, 0xa200).is_none());
            kernel.modules[0].sha256 = "bbb".into();
            kernel.modules[0].cro.name = "untranslated".into();
            assert!(cache.resolve(&kernel, &modules, ENTRIES, 0xa200).is_none());
            kernel.modules[0].cro.name = "second".into();
            kernel.modules[0].cro.file_size = 0x100;
            assert!(cache.resolve(&kernel, &modules, ENTRIES, 0xa200).is_none());
            kernel.modules[0].cro.file_size = 0x1000;
            assert!(cache.resolve(&kernel, &modules, ENTRIES, 0xa200).is_some());
            assert!(cache.resolve(&kernel, &modules, ENTRIES, 0xa201).is_none());
            // Replacing the complete table, restoring an unchanged clone and
            // mutating through the exposed Vec must all preserve ownership.
            let previous = kernel.modules.clone();
            kernel.modules = vec![loaded("first", "aaa", 0xa000)].into();
            let (code, _) = cache.resolve(&kernel, &modules, ENTRIES, 0xa200).unwrap();
            code(&mut cpu, &mut mem).unwrap();
            assert_eq!(cpu.r[0], 1);
            kernel.modules = previous;
            let (code, _) = cache.resolve(&kernel, &modules, ENTRIES, 0xa200).unwrap();
            code(&mut cpu, &mut mem).unwrap();
            assert_eq!(cpu.r[0], 2);
            let mut replacement = vec![loaded("first", "aaa", 0xa000)];
            std::mem::swap(&mut *kernel.modules, &mut replacement);
            let (code, _) = cache.resolve(&kernel, &modules, ENTRIES, 0xa200).unwrap();
            code(&mut cpu, &mut mem).unwrap();
            assert_eq!(cpu.r[0], 1);
            kernel.modules.clear();
            assert!(cache.resolve(&kernel, &modules, ENTRIES, 0xa200).is_none());
        }
    }
}

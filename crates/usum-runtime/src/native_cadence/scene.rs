use std::sync::Arc;

use usum_guest::Memory;

use super::{engine_manager, set_scene_covered};
use crate::kernel::Kernel;

const PENDING: u32 = 8;
const CURRENT: u32 = 9;
const HIGHER: u8 = 0;
const NORMAL: u8 = 1;
const NONE_PENDING: u8 = 2;

pub(super) fn request_higher(mem: &mut Memory, manager: u32) -> bool {
    mem.read_u8(manager + CURRENT).ok() == Some(NORMAL)
        && mem.read_u8(manager + PENDING).ok() == Some(NONE_PENDING)
        && mem.write_u8(manager + PENDING, HIGHER).is_ok()
}

/// The dispatcher owns the lifetime of its request to the guest engine.
/// The game may already have selected a higher rate for its own reasons.
#[derive(Default)]
pub(crate) struct SceneRate {
    revision: Option<Arc<()>>,
    covered: bool,
    requested_for: Option<u32>,
}

impl SceneRate {
    pub(crate) fn refresh(&mut self, kernel: &Kernel, mem: &mut Memory, retry: bool) {
        let revision = kernel.modules.revision();
        let changed = self.revision.as_ref().is_none_or(|old| !Arc::ptr_eq(old, revision));
        if changed {
            self.revision = Some(Arc::clone(revision));
            let mut field = false;
            let mut excluded = false;
            for module in kernel.modules.iter().filter(|module| module.base != 0) {
                match module.cro.name.as_str() {
                    "FieldRo" => field = true,
                    "MovieLib" | "Battle" => excluded = true,
                    _ => {}
                }
            }
            self.covered = field && !excluded;
            set_scene_covered(self.covered);
        }
        if (changed || retry) && (self.covered || self.requested_for.is_some()) {
            let manager = engine_manager(mem);
            self.sync_manager(mem, manager);
        }
    }

    fn sync_manager(&mut self, mem: &mut Memory, manager: Option<u32>) {
        let Some(manager) = manager else { return };
        if self.requested_for != Some(manager) {
            self.requested_for = None;
        }
        if self.covered {
            if request_higher(mem, manager) {
                self.requested_for = Some(manager);
            }
            return;
        }
        if self.requested_for.is_none() {
            return;
        }

        let current = mem.read_u8(manager + CURRENT).ok();
        let pending = mem.read_u8(manager + PENDING).ok();
        let released = match (current, pending) {
            // Cancel our request if the engine has not acted on it.
            (Some(NORMAL), Some(HIGHER)) => mem.write_u8(manager + PENDING, NONE_PENDING).is_ok(),
            (Some(HIGHER), Some(NONE_PENDING)) => mem.write_u8(manager + PENDING, NORMAL).is_ok(),
            // The engine already has a normal-rate request, or is back at normal.
            (_, Some(NORMAL)) | (Some(NORMAL), Some(NONE_PENDING)) => true,
            _ => false,
        };
        if released {
            self.requested_for = None;
        }
    }

    pub(crate) fn reset(&mut self) {
        self.revision = None;
        self.covered = false;
        self.requested_for = None;
        set_scene_covered(false);
    }
}

impl Drop for SceneRate {
    fn drop(&mut self) {
        set_scene_covered(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ro::Loaded;
    use usum_guest::Perm;
    use usum_import::cro::Cro;

    fn module(name: &str) -> Loaded {
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
            base: 0x1000,
            unresolved: vec![],
            image: 0x1000,
            sha256: String::new(),
        }
    }

    fn context() -> (Memory, u32) {
        let profile = super::super::ULTRA_SUN;
        let mut mem = Memory::new();
        let manager: u32 = 0x2000;
        mem.map(profile.update_start.0, profile.update_start.1.to_le_bytes().to_vec(), Perm::R).unwrap();
        mem.map(profile.manager, manager.to_le_bytes().to_vec(), Perm::R).unwrap();
        mem.map(manager, vec![0; 12], Perm::RW).unwrap();
        mem.write_u8(manager + PENDING, NONE_PENDING).unwrap();
        (mem, manager)
    }

    #[test]
    fn game_owned_movie_rate_is_untouched_and_field_request_is_released_for_battle() {
        let (mut mem, manager) = context();
        let mut kernel = Kernel::default();
        let mut rate = SceneRate::default();
        kernel.modules.extend([module("FieldRo"), module("MovieLib")]);
        rate.refresh(&kernel, &mut mem, false);
        assert_eq!(mem.read_u8(manager + PENDING).unwrap(), NONE_PENDING);
        assert!(!rate.covered);

        kernel.modules.pop();
        rate.refresh(&kernel, &mut mem, false);
        assert!(rate.covered);
        assert!(rate.requested_for.is_none());
        mem.write_u8(manager + CURRENT, NORMAL).unwrap();
        rate.refresh(&kernel, &mut mem, false);
        assert_eq!(mem.read_u8(manager + PENDING).unwrap(), NONE_PENDING);
        rate.refresh(&kernel, &mut mem, true);
        assert_eq!(mem.read_u8(manager + PENDING).unwrap(), HIGHER);
        assert_eq!(rate.requested_for, Some(manager));

        mem.write_u8(manager + CURRENT, HIGHER).unwrap();
        mem.write_u8(manager + PENDING, NONE_PENDING).unwrap();
        kernel.modules.push(module("Battle"));
        rate.refresh(&kernel, &mut mem, false);
        assert!(!rate.covered);
        assert_eq!(mem.read_u8(manager + PENDING).unwrap(), NORMAL);
        assert!(rate.requested_for.is_none());
    }

    #[test]
    fn pending_request_is_cancelled_on_exit_and_restart_discards_ownership() {
        let (mut mem, manager) = context();
        let mut kernel = Kernel::default();
        let mut rate = SceneRate::default();
        mem.write_u8(manager + CURRENT, NORMAL).unwrap();
        kernel.modules.push(module("FieldRo"));
        rate.refresh(&kernel, &mut mem, false);
        assert_eq!(mem.read_u8(manager + PENDING).unwrap(), HIGHER);
        kernel.modules.clear();
        rate.refresh(&kernel, &mut mem, false);
        assert_eq!(mem.read_u8(manager + PENDING).unwrap(), NONE_PENDING);

        kernel.modules.push(module("FieldRo"));
        rate.refresh(&kernel, &mut mem, false);
        assert_eq!(rate.requested_for, Some(manager));
        rate.reset();
        kernel.modules.clear();
        mem.write_u8(manager + CURRENT, HIGHER).unwrap();
        mem.write_u8(manager + PENDING, NONE_PENDING).unwrap();
        rate.refresh(&kernel, &mut mem, false);
        assert_eq!(mem.read_u8(manager + PENDING).unwrap(), NONE_PENDING);
    }
}

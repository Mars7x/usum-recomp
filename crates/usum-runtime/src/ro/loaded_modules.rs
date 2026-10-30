//! Track every mutable borrow of loaded-module metadata.
use super::Loaded;
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

/// Loaded modules and a revision that changes before mutable access escapes.
/// Clones share a revision while their contents agree; mutating either clone
/// creates a fresh token. Cached tokens keep old allocations alive, preventing
/// address reuse from validating stale code.
#[derive(Clone, Debug, Default)]
pub struct LoadedModules {
    entries: Vec<Loaded>,
    revision: Arc<()>,
}

impl LoadedModules {
    pub(crate) fn revision(&self) -> &Arc<()> {
        &self.revision
    }
}
impl From<Vec<Loaded>> for LoadedModules {
    fn from(entries: Vec<Loaded>) -> Self {
        Self { entries, revision: Arc::new(()) }
    }
}
impl Deref for LoadedModules {
    type Target = Vec<Loaded>;
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}
impl DerefMut for LoadedModules {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.revision = Arc::new(());
        &mut self.entries
    }
}
impl<'a> IntoIterator for &'a LoadedModules {
    type Item = &'a Loaded;
    type IntoIter = std::slice::Iter<'a, Loaded>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}
impl<'a> IntoIterator for &'a mut LoadedModules {
    type Item = &'a mut Loaded;
    type IntoIter = std::slice::IterMut<'a, Loaded>;
    fn into_iter(self) -> Self::IntoIter {
        self.deref_mut().iter_mut()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn immutable_access_and_clones_share_revision_until_mutable_access() {
        let mut modules = LoadedModules::default();
        let initial = Arc::clone(modules.revision());
        assert!(modules.is_empty());
        assert!(Arc::ptr_eq(&initial, modules.revision()));
        let mut copy = modules.clone();
        assert!(Arc::ptr_eq(&initial, copy.revision()));
        copy.clear();
        assert!(!Arc::ptr_eq(&initial, copy.revision()));
        assert!(Arc::ptr_eq(&initial, modules.revision()));
        let mut replacement = Vec::new();
        std::mem::swap(&mut *modules, &mut replacement);
        assert!(!Arc::ptr_eq(&initial, modules.revision()));
        assert!(!Arc::ptr_eq(copy.revision(), modules.revision()));
        let before_iteration = Arc::clone(modules.revision());
        for _ in &mut modules {}
        assert!(!Arc::ptr_eq(&before_iteration, modules.revision()));
    }
}

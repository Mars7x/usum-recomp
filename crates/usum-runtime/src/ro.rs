//! RO's work on loaded modules (ADR-0007): where each segment of each
//! module lives, applying relocations, and linking imports to exports.
//!
//! Relocation values follow 3dbrew "CRO0" (`writeRelocation`): ABS32 and
//! TARGET1 store base + addend; REL32 base + addend - place; PREL31 the
//! same in 31 bits; CALL, JUMP24 and THM_PC22 encode a branch to base +
//! addend (CALL and THM_PC22 switch to BLX when the target's instruction
//! set differs; JUMP24 cannot). Branch ranges are checked as 3dbrew says
//! RO does (±32 MB ARM, ±4 MB Thumb).
//!
//! Not modelled: what the fix level frees, and RO's rewrite of the mapped
//! header (the game has not been seen to read either).

mod loaded_modules;
pub use loaded_modules::LoadedModules;

use usum_guest::Memory;
use usum_import::cro::{self, Cro, ImportRef, Relocation, SegmentKind, SegmentOffset};

/// A module RO has loaded: its parsed tables and where its segments are.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub cro: Cro,
    /// Guest address of each segment (index as in the segment table).
    pub segments: Vec<u32>,
    /// Where the module is mapped (its header); the CRS: 0.
    pub base: u32,
    /// Imports not resolved yet, by index into `cro.imports`.
    pub unresolved: Vec<usize>,
    /// Where the module's file image is mapped (a CRO: `base`; the CRS:
    /// where RO Initialize mapped it). Its tables are patched there.
    pub image: u32,
    /// SHA-256 of the module file as the game passed it (before any
    /// relocation): which translation, if any, is this module's.
    pub sha256: String,
}

impl Loaded {
    pub fn address(&self, at: SegmentOffset) -> Option<u32> {
        self.segments.get(usize::from(at.segment)).map(|s| s.wrapping_add(at.offset))
    }

    /// Where the bytes at `at` are while RO patches them. For .data that
    /// is the mapped image, not the game's .data buffer: after LoadCRO the
    /// game copies .data from the image into its buffer itself (loader at
    /// 0x00172FF4), so patches made in the buffer would be overwritten
    /// (R-033). Everything else is patched where it is used.
    pub fn patch_address(&self, at: SegmentOffset) -> Option<u32> {
        let s = self.cro.segments.get(usize::from(at.segment))?;
        if s.kind == SegmentKind::Data && !self.cro.is_crs {
            Some(self.base.wrapping_add(s.offset).wrapping_add(at.offset))
        } else {
            self.address(at)
        }
    }

    /// The module's text range (for dispatch and reports).
    pub fn text(&self) -> Option<std::ops::Range<u32>> {
        let (i, s) = self.cro.segments.iter().enumerate().find(|(_, s)| s.kind == SegmentKind::Text && s.size > 0)?;
        Some(self.segments[i]..self.segments[i] + s.size)
    }

    fn export(&self, name: &str) -> Option<u32> {
        self.cro.named_exports.iter().find(|e| e.name == name).and_then(|e| self.address(e.at))
    }
}

/// Segment addresses: text and rodata inside the mapped image (from
/// `base`), data and bss in the buffers the game gave (3dbrew "RO:
/// LoadExeCRO": "all CRO .data section addresses are automatically
/// adjusted to use this address", the same for .bss). A CRS's segments
/// are the executable's own addresses.
pub fn place(c: &Cro, base: u32, data: u32, bss: u32) -> Vec<u32> {
    c.segments
        .iter()
        .map(|s| match s.kind {
            _ if c.is_crs => s.offset,
            SegmentKind::Data => data,
            SegmentKind::Bss => bss,
            _ => base.wrapping_add(s.offset),
        })
        .collect()
}

fn branch_range(offset: i64, limit: i64) -> Result<u32, String> {
    if offset < -limit || offset >= limit {
        Err(format!("branch relocation out of range ({offset:#x})"))
    } else {
        Ok(offset as u32)
    }
}

/// The new value at `place` (holding `old`) for one relocation to
/// `target` (base + addend; bit 0 set for Thumb code). Two halfwords for
/// THM_PC22, as one little-endian word.
pub fn relocated(kind: u8, target: u32, place: u32, old: u32) -> Result<Option<u32>, String> {
    let thumb = target & 1 != 0;
    let to = target & !1;
    Ok(Some(match kind {
        cro::R_ARM_NONE => return Ok(None),
        cro::R_ARM_ABS32 | cro::R_ARM_TARGET1 => target,
        cro::R_ARM_REL32 => target.wrapping_sub(place),
        cro::R_ARM_PREL31 => (old & 0x8000_0000) | (target.wrapping_sub(place) & 0x7FFF_FFFF),
        cro::R_ARM_CALL => {
            let offset = branch_range(i64::from(to) - i64::from(place) - 8, 1 << 25)?;
            if thumb {
                // BLX immediate: H carries bit 1 of the offset
                0xFA00_0000 | ((offset >> 1) & 1) << 24 | (offset >> 2) & 0x00FF_FFFF
            } else {
                (old & 0xF000_0000) | 0x0B00_0000 | (offset >> 2) & 0x00FF_FFFF
            }
        }
        cro::R_ARM_JUMP24 => {
            if thumb {
                return Err("JUMP24 relocation to Thumb code".into());
            }
            let offset = branch_range(i64::from(to) - i64::from(place) - 8, 1 << 25)?;
            (old & 0xFF00_0000) | (offset >> 2) & 0x00FF_FFFF
        }
        cro::R_ARM_THM_PC22 => {
            // BL to Thumb, BLX to ARM (from Align(PC, 4)); Thumb-1 22-bit form
            let pc = place.wrapping_add(4);
            let from = if thumb { pc } else { pc & !3 };
            let offset = branch_range(i64::from(to) - i64::from(from), 1 << 22)?;
            let high = 0xF000 | (offset >> 12) & 0x7FF;
            let low = if thumb { 0xF800 | (offset >> 1) & 0x7FF } else { 0xE800 | (offset >> 1) & 0x7FE };
            high | low << 16
        }
        other => return Err(format!("relocation type {other}")),
    }))
}

/// Applies `r` with `target` (base + addend) in guest memory, through
/// the module's segment addresses. Device writes: RO writes code that the
/// process sees read-only.
pub fn apply(mem: &mut Memory, module: &Loaded, r: &Relocation, base: u32) -> Result<(), String> {
    // the address the value is relative to, and where it is written
    let place = module.address(r.target).ok_or_else(|| format!("relocation into segment {}", r.target.segment))?;
    let at = module.patch_address(r.target).ok_or_else(|| format!("relocation into segment {}", r.target.segment))?;
    let target = base.wrapping_add(r.addend);
    let old = mem.read_u32(at).map_err(|f| format!("relocation at {at:#x}: {f}"))?;
    if let Some(value) = relocated(r.kind, target, place, old)? {
        mem.device_write(at, &value.to_le_bytes()).map_err(|f| format!("relocation at {at:#x}: {f}"))?;
    }
    Ok(())
}

/// The internal relocations of `module`: the referred segment's address
/// is the base.
pub fn relocate_internal(mem: &mut Memory, module: &Loaded) -> Result<usize, String> {
    for r in &module.cro.internal {
        let base = *module.segments.get(usize::from(r.segment_or_last)).ok_or("internal relocation to a missing segment")?;
        apply(mem, module, r, base)?;
    }
    Ok(module.cro.internal.len())
}

/// What an import refers to, if a loaded module exports it.
pub fn resolve(modules: &[Loaded], what: &ImportRef) -> Option<u32> {
    match what {
        ImportRef::Named(name) => modules.iter().find_map(|m| m.export(name)),
        ImportRef::Indexed { module, index } => {
            let m = modules.iter().find(|m| &m.cro.name == module)?;
            m.address(*m.cro.indexed_exports.get(*index as usize)?)
        }
        ImportRef::Anonymous { module, at } => modules.iter().find(|m| &m.cro.name == module)?.address(*at),
    }
}

/// Marks import `k` of `module` resolved or not: byte 6 of the first entry
/// of its relocation list, which 3dbrew says RO sets "to 1 on the first
/// entry if all symbols loaded". Ultra Sun's loader checks it after
/// linking (0x00173A60) and panics if any list is unmarked (R-034).
pub fn mark(mem: &mut Memory, module: &Loaded, k: usize, resolved: bool) -> Result<(), String> {
    let at = module.image + module.cro.import_table + 12 * module.cro.imports[k].head + 6;
    mem.device_write(at, &[u8::from(resolved)]).map_err(|f| format!("{} import status: {f}", module.cro.name))
}

/// Links module `i`'s pending imports to what `modules` export now.
/// Returns how many resolved.
pub fn link(mem: &mut Memory, modules: &mut [Loaded], i: usize) -> Result<usize, String> {
    let pending = std::mem::take(&mut modules[i].unresolved);
    let mut still = Vec::new();
    let mut done = 0;
    for k in pending {
        let what = modules[i].cro.imports[k].what.clone();
        match resolve(modules, &what) {
            Some(address) => {
                let m = &modules[i];
                for r in &m.cro.imports[k].relocations {
                    apply(mem, m, r, address)?;
                }
                mark(mem, m, k, true)?;
                done += 1;
            }
            None => still.push(k),
        }
    }
    modules[i].unresolved = still;
    Ok(done)
}

/// Points module `i`'s unresolved imports at its OnUnresolved function,
/// if it has one (where calls to missing symbols go). They stay pending:
/// a module loaded later may export them.
pub fn bind_unresolved(mem: &mut Memory, module: &Loaded) -> Result<(), String> {
    let Some(fallback) = module.cro.on_unresolved.and_then(|at| module.address(at)) else { return Ok(()) };
    for &k in &module.unresolved {
        for r in &module.cro.imports[k].relocations {
            apply(mem, module, r, fallback)?;
        }
    }
    Ok(())
}

/// After module `gone` is removed from `modules`: every import that
/// resolved into its address range goes back to unresolved and to its
/// module's OnUnresolved (where calls to missing symbols go).
pub fn forget(mem: &mut Memory, modules: &mut [Loaded], gone: &Loaded) -> Result<(), String> {
    let range = gone.base..gone.base + gone.cro.file_size;
    let in_gone = |a: u32| range.contains(&a) || gone.segments.contains(&a);
    for m in modules.iter_mut() {
        let mut lost = Vec::new();
        for (k, import) in m.cro.imports.iter().enumerate() {
            if m.unresolved.contains(&k) {
                continue;
            }
            // what it resolved to, if that was the departing module
            if resolve(std::slice::from_ref(gone), &import.what).is_some_and(in_gone) {
                lost.push(k);
            }
        }
        if lost.is_empty() {
            continue;
        }
        m.unresolved.extend(&lost);
        let m = &*m;
        for &k in &lost {
            mark(mem, m, k, false)?;
        }
        if let Some(fallback) = m.cro.on_unresolved.and_then(|at| m.address(at)) {
            for &k in &lost {
                for r in &m.cro.imports[k].relocations {
                    apply(mem, m, r, fallback)?;
                }
            }
        }
    }
    Ok(())
}

/// Header fields holding file offsets that RO turns into addresses (from
/// the mapped base) when it loads a module: the name, code, .data and
/// module name offsets, then every table offset (3dbrew "CRO0" header).
/// That RO does so is a HYPOTHESIS (R-030): after LoadCRO, Ultra Sun's
/// loader (0x00172FF4) reads the .data offset at 0xB8 and uses it as an
/// address in the mapped image.
pub const REBASED_HEADER_FIELDS: [usize; 18] =
    [0x84, 0xB0, 0xB8, 0xC0, 0xC8, 0xD0, 0xD8, 0xE0, 0xE8, 0xF0, 0xF8, 0x100, 0x108, 0x110, 0x118, 0x120, 0x128, 0x130];

/// Rewrites the mapped header of `module`: offsets to addresses, each
/// segment table entry's offset to the segment's address, and the magic
/// to "FIXD" for a fix level above 0 (3dbrew).
pub fn rebase_header(mem: &mut Memory, module: &Loaded, fix_level: u32) -> Result<(), String> {
    let base = module.base;
    let fault = |f: usum_guest::Fault| format!("{} header: {f}", module.cro.name);
    for at in REBASED_HEADER_FIELDS {
        let v = mem.read_u32(base + at as u32).map_err(fault)?;
        if v != 0 {
            mem.device_write(base + at as u32, &base.wrapping_add(v).to_le_bytes()).map_err(fault)?;
        }
    }
    // the segment table's offset field was rebased just above
    let table = mem.read_u32(base + 0xC8).map_err(fault)?;
    for (k, address) in module.segments.iter().enumerate() {
        mem.device_write(table + 12 * k as u32, &address.to_le_bytes()).map_err(fault)?;
    }
    if fix_level != 0 {
        mem.device_write(base + 0x80, b"FIXD").map_err(fault)?;
    }
    Ok(())
}

/// Undoes `rebase_header` when the module unloads: offsets again, the
/// segment table back to file offsets, the magic back to "CRO0". After
/// UnloadCRO, Ultra Sun adds the buffer address to a header field it reads
/// back (0x001731D4), so it expects offsets there again (R-033).
pub fn unrebase_header(mem: &mut Memory, module: &Loaded) -> Result<(), String> {
    let base = module.base;
    let fault = |f: usum_guest::Fault| format!("{} header: {f}", module.cro.name);
    let table = mem.read_u32(base + 0xC8).map_err(fault)?;
    for (k, s) in module.cro.segments.iter().enumerate() {
        mem.device_write(table + 12 * k as u32, &s.offset.to_le_bytes()).map_err(fault)?;
    }
    for at in REBASED_HEADER_FIELDS {
        let v = mem.read_u32(base + at as u32).map_err(fault)?;
        if v != 0 {
            mem.device_write(base + at as u32, &v.wrapping_sub(base).to_le_bytes()).map_err(fault)?;
        }
    }
    mem.device_write(base + 0x80, b"CRO0").map_err(fault)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_relocations_encode_like_the_assembler() {
        // `bl` from 0x1000 to 0x2000 (ARM): offset (0x2000 - 0x1008) >> 2
        assert_eq!(relocated(cro::R_ARM_CALL, 0x2000, 0x1000, 0xEB00_0000), Ok(Some(0xEB00_03FE)));
        // `blx` to Thumb 0x2002: H = 1
        assert_eq!(relocated(cro::R_ARM_CALL, 0x2003, 0x1000, 0xEB00_0000), Ok(Some(0xFB00_03FE)));
        // Thumb `bl` from 0x1000 to Thumb 0x1100: offset 0xFC
        assert_eq!(relocated(cro::R_ARM_THM_PC22, 0x1101, 0x1000, 0), Ok(Some(0xF87E_F000)));
        assert!(relocated(cro::R_ARM_JUMP24, 0x2001, 0x1000, 0xEA00_0000).is_err());
        assert!(relocated(cro::R_ARM_THM_PC22, 0x0100_1000, 0x1000, 0).is_err(), "beyond 4 MB");
    }

    #[test]
    fn data_relocations() {
        assert_eq!(relocated(cro::R_ARM_ABS32, 0x1234, 0x1000, 0), Ok(Some(0x1234)));
        assert_eq!(relocated(cro::R_ARM_REL32, 0x1234, 0x1000, 0), Ok(Some(0x234)));
        assert_eq!(relocated(cro::R_ARM_PREL31, 0x0FF0, 0x1000, 0x8000_0000), Ok(Some(0xFFFF_FFF0)));
    }
}

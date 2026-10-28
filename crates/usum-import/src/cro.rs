//! CRO and CRS modules (3dbrew "CRO0"): dynamically loaded code, and
//! the main executable's symbol module. Parsed as stored in the RomFS,
//! before RO fixes them up.
//!
//! All table offsets are file offsets. A "segment offset" is a segment
//! index in bits 0-3 and an offset into that segment in bits 4-31.

use serde::Serialize;

use crate::source::Le;
use crate::Error;

const HEADER_SIZE: usize = 0x138;
/// Not set: OnLoad and the other entry points, and nnroControlObject.
pub const NONE: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SegmentKind {
    Text,
    Rodata,
    Data,
    Bss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Segment {
    /// File offset (bss: none; a CRS: the executable's address). Unused
    /// entries are empty, at 0, of type text (measured: every module
    /// of both games has five entries).
    pub offset: u32,
    pub size: u32,
    pub kind: SegmentKind,
}

/// A place in a segment: (segment index, offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SegmentOffset {
    pub segment: u8,
    pub offset: u32,
}

impl SegmentOffset {
    pub fn decode(raw: u32) -> SegmentOffset {
        SegmentOffset { segment: (raw & 0xF) as u8, offset: raw >> 4 }
    }
}

/// One relocation (12 bytes): where, type, the table-specific byte, and
/// the addend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Relocation {
    pub target: SegmentOffset,
    /// R_ARM_* (2 ABS32, 3 REL32, 10 THM_PC22, 28 CALL, 29 JUMP24,
    /// 38 TARGET1, 42 PREL31; 0 none).
    pub kind: u8,
    /// Internal relocations: the segment the value refers to. Import
    /// relocations: non-zero on the last of a list.
    pub segment_or_last: u8,
    pub addend: u32,
}

pub const R_ARM_NONE: u8 = 0;
pub const R_ARM_ABS32: u8 = 2;
pub const R_ARM_REL32: u8 = 3;
pub const R_ARM_THM_PC22: u8 = 10;
pub const R_ARM_CALL: u8 = 28;
pub const R_ARM_JUMP24: u8 = 29;
pub const R_ARM_TARGET1: u8 = 38;
pub const R_ARM_PREL31: u8 = 42;
pub const KNOWN_RELOCATIONS: [u8; 8] =
    [R_ARM_NONE, R_ARM_ABS32, R_ARM_REL32, R_ARM_THM_PC22, R_ARM_CALL, R_ARM_JUMP24, R_ARM_TARGET1, R_ARM_PREL31];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NamedExport {
    pub name: String,
    pub at: SegmentOffset,
}

/// What a list of import relocations refers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ImportRef {
    Named(String),
    /// From `module`, its indexed export number `index`.
    Indexed {
        module: String,
        index: u32,
    },
    /// From `module`, the export at a segment offset in that module.
    Anonymous {
        module: String,
        at: SegmentOffset,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Import {
    pub what: ImportRef,
    /// The index of the list's first entry in the import relocation
    /// table: RO marks it resolved there (byte 6, 3dbrew).
    pub head: u32,
    /// The patches to apply once it resolves.
    pub relocations: Vec<Relocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cro {
    pub name: String,
    /// The main executable's symbol module (CRS), not loadable code.
    pub is_crs: bool,
    pub file_size: u32,
    pub bss_size: u32,
    pub segments: Vec<Segment>,
    pub on_load: Option<SegmentOffset>,
    pub on_exit: Option<SegmentOffset>,
    pub on_unresolved: Option<SegmentOffset>,
    pub named_exports: Vec<NamedExport>,
    pub indexed_exports: Vec<SegmentOffset>,
    pub imports: Vec<Import>,
    /// Relocations within the module itself.
    pub internal: Vec<Relocation>,
    /// The import relocation table's file offset (12-byte entries).
    pub import_table: u32,
}

/// A table at (header offset field, count field), `entry` bytes each,
/// as a reader over its bytes.
fn table<'a>(h: &Le<'a>, bytes: &'a [u8], at: usize, entry: usize) -> Result<(Le<'a>, usize), Error> {
    let offset = h.u32(at)? as usize;
    let count = h.u32(at + 4)? as usize;
    let len = count.checked_mul(entry).ok_or_else(|| Error::malformed("CRO", "table size overflows"))?;
    let end = offset.checked_add(len).filter(|&e| e <= bytes.len());
    end.ok_or_else(|| Error::malformed("CRO", format!("table at header {at:#x} lies outside the file")))?;
    Ok((Le::new("CRO table", &bytes[offset..offset + len]), count))
}

fn string(bytes: &[u8], offset: u32) -> Result<String, Error> {
    let tail = bytes.get(offset as usize..).ok_or_else(|| Error::malformed("CRO", "string outside the file"))?;
    let end = tail.iter().position(|&b| b == 0).ok_or_else(|| Error::malformed("CRO", "unterminated string"))?;
    String::from_utf8(tail[..end].to_vec()).map_err(|_| Error::malformed("CRO", "string is not UTF-8"))
}

fn relocation(t: &Le, i: usize) -> Result<Relocation, Error> {
    let at = 12 * i;
    Ok(Relocation {
        target: SegmentOffset::decode(t.u32(at)?),
        kind: t.u8(at + 4)?,
        segment_or_last: t.u8(at + 5)?,
        addend: t.u32(at + 8)?,
    })
}

fn entry_point(raw: u32) -> Option<SegmentOffset> {
    (raw != NONE).then(|| SegmentOffset::decode(raw))
}

pub fn parse(bytes: &[u8]) -> Result<Cro, Error> {
    if bytes.len() < HEADER_SIZE {
        return Err(Error::malformed("CRO", "shorter than its header"));
    }
    let h = Le::new("CRO header", bytes);
    if h.slice(0x80, 4)? != b"CRO0" {
        return Err(Error::malformed("CRO", "no CRO0 magic (fixed already, or not a module)"));
    }
    let file_size = h.u32(0x90)?;
    if file_size as usize != bytes.len() {
        return Err(Error::malformed("CRO", format!("header size {file_size:#x}, file {:#x}", bytes.len())));
    }
    let name = string(bytes, h.u32(0xC0)?)?;
    let is_crs = h.u32(0xA0)? == NONE;

    let (t, n) = table(&h, bytes, 0xC8, 12)?;
    let mut segments = Vec::with_capacity(n);
    for i in 0..n {
        let kind = match t.u32(12 * i + 8)? {
            0 => SegmentKind::Text,
            1 => SegmentKind::Rodata,
            2 => SegmentKind::Data,
            3 => SegmentKind::Bss,
            other => return Err(Error::malformed("CRO", format!("segment type {other}"))),
        };
        let (offset, size) = (t.u32(12 * i)?, t.u32(12 * i + 4)?);
        // a CRS describes the executable's segments, not ranges of itself
        let in_file =
            is_crs || kind == SegmentKind::Bss || size == 0 || u64::from(offset) + u64::from(size) <= u64::from(file_size);
        if !in_file {
            return Err(Error::malformed("CRO", format!("segment {i} lies outside the file")));
        }
        segments.push(Segment { offset, size, kind });
    }
    let valid = |at: SegmentOffset| at.segment_ok(&segments);

    let (t, n) = table(&h, bytes, 0xD0, 8)?;
    let mut named_exports = Vec::with_capacity(n);
    for i in 0..n {
        let at = SegmentOffset::decode(t.u32(8 * i + 4)?);
        if !valid(at) {
            return Err(Error::malformed("CRO", format!("named export {i} outside its segment")));
        }
        named_exports.push(NamedExport { name: string(bytes, t.u32(8 * i)?)?, at });
    }
    let (t, n) = table(&h, bytes, 0xD8, 4)?;
    let indexed_exports = (0..n).map(|i| t.u32(4 * i).map(SegmentOffset::decode)).collect::<Result<Vec<_>, _>>()?;

    // import relocation lists: a head (file offset into the table) runs
    // until an entry marked last
    let (relocs, nrelocs) = table(&h, bytes, 0xF8, 12)?;
    let relocs_base = h.u32(0xF8)?;
    let head_index = |head: u32| -> Result<usize, Error> {
        let first = head.checked_sub(relocs_base).filter(|d| d % 12 == 0).map(|d| (d / 12) as usize);
        first.filter(|&i| i < nrelocs).ok_or_else(|| Error::malformed("CRO", "import list head outside its table"))
    };
    let list = |head: u32| -> Result<Vec<Relocation>, Error> {
        let first = head_index(head)?;
        let mut out = Vec::new();
        for i in first..nrelocs {
            let r = relocation(&relocs, i)?;
            out.push(r);
            if r.segment_or_last != 0 {
                return Ok(out);
            }
        }
        Err(Error::malformed("CRO", "import list runs off its table"))
    };
    let mut imports = Vec::new();
    let (t, n) = table(&h, bytes, 0x100, 8)?;
    for i in 0..n {
        let head = t.u32(8 * i + 4)?;
        imports.push(Import {
            what: ImportRef::Named(string(bytes, t.u32(8 * i)?)?),
            head: head_index(head)? as u32,
            relocations: list(head)?,
        });
    }
    let (modules, n) = table(&h, bytes, 0xF0, 20)?;
    let indexed_base = h.u32(0x108)?;
    let anonymous_base = h.u32(0x110)?;
    let (indexed, nindexed) = table(&h, bytes, 0x108, 8)?;
    let (anonymous, nanonymous) = table(&h, bytes, 0x110, 8)?;
    for m in 0..n {
        let module = string(bytes, modules.u32(20 * m)?)?;
        let sub = |base: u32, head: u32, count: u32, total: usize| -> Result<std::ops::Range<usize>, Error> {
            let first = head.checked_sub(base).filter(|d| d % 8 == 0).map(|d| (d / 8) as usize);
            let range = first.map(|f| f..f + count as usize).filter(|r| r.end <= total);
            range.ok_or_else(|| Error::malformed("CRO", format!("import list of {module} outside its table")))
        };
        for i in sub(indexed_base, modules.u32(20 * m + 4)?, modules.u32(20 * m + 8)?, nindexed)? {
            let what = ImportRef::Indexed { module: module.clone(), index: indexed.u32(8 * i)? };
            let head = indexed.u32(8 * i + 4)?;
            imports.push(Import { what, head: head_index(head)? as u32, relocations: list(head)? });
        }
        for i in sub(anonymous_base, modules.u32(20 * m + 12)?, modules.u32(20 * m + 16)?, nanonymous)? {
            let what = ImportRef::Anonymous { module: module.clone(), at: SegmentOffset::decode(anonymous.u32(8 * i)?) };
            let head = anonymous.u32(8 * i + 4)?;
            imports.push(Import { what, head: head_index(head)? as u32, relocations: list(head)? });
        }
    }

    let (t, n) = table(&h, bytes, 0x128, 12)?;
    let internal = (0..n).map(|i| relocation(&t, i)).collect::<Result<Vec<_>, _>>()?;
    for r in imports.iter().flat_map(|i| &i.relocations).chain(&internal) {
        if !valid(r.target) || !KNOWN_RELOCATIONS.contains(&r.kind) {
            return Err(Error::malformed("CRO", format!("relocation {r:?}")));
        }
    }

    Ok(Cro {
        name,
        is_crs,
        file_size,
        bss_size: h.u32(0x94)?,
        segments,
        on_load: entry_point(h.u32(0xA4)?),
        on_exit: entry_point(h.u32(0xA8)?),
        on_unresolved: entry_point(h.u32(0xAC)?),
        named_exports,
        indexed_exports,
        imports,
        internal,
        import_table: relocs_base,
    })
}

impl Cro {
    /// The text segment: (index, file offset, size), if any.
    pub fn text(&self) -> Option<(usize, u32, u32)> {
        self.segments
            .iter()
            .enumerate()
            .find(|(_, s)| s.kind == SegmentKind::Text && s.size > 0)
            .map(|(i, s)| (i, s.offset, s.size))
    }

    /// Code this module itself says is entered, as offsets from the start
    /// of the file (bit 0 set for Thumb): exports in text, OnLoad, OnExit,
    /// OnUnresolved, and every internal relocation that points into text
    /// (a code address the module stores). For a module that is not yet
    /// relocated these are the only reliable code pointers.
    pub fn code_seeds(&self) -> Vec<u32> {
        let Some((text, start, _)) = self.text() else { return Vec::new() };
        let in_text = |at: &SegmentOffset| (usize::from(at.segment) == text).then(|| start.wrapping_add(at.offset));
        let mut seeds: Vec<u32> = self
            .named_exports
            .iter()
            .map(|e| e.at)
            .chain(self.indexed_exports.iter().copied())
            .filter_map(|at| in_text(&at))
            .collect();
        seeds.extend([self.on_load, self.on_exit, self.on_unresolved].into_iter().flatten().filter_map(|at| in_text(&at)));
        seeds.extend(
            self.internal.iter().filter(|r| usize::from(r.segment_or_last) == text).map(|r| start.wrapping_add(r.addend)),
        );
        seeds.sort_unstable();
        seeds.dedup();
        seeds
    }
}

impl SegmentOffset {
    /// Inside one of `segments` (or at its end: an empty segment's start).
    fn segment_ok(self, segments: &[Segment]) -> bool {
        segments.get(usize::from(self.segment)).is_some_and(|s| self.offset <= s.size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal module: one text segment of 8 bytes, one named export,
    /// one internal relocation, one named import with two patches.
    fn synthetic() -> Vec<u8> {
        let mut b = vec![0u8; 0x200];
        let put = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"CRO0");
        put(&mut b, 0x90, 0x200);
        put(&mut b, 0xA0, 0);
        for at in [0xA4, 0xA8, 0xAC] {
            put(&mut b, at, NONE);
        }
        b[0x180..0x185].copy_from_slice(b"Test\0");
        b[0x188..0x18C].copy_from_slice(b"foo\0");
        b[0x190..0x194].copy_from_slice(b"bar\0");
        put(&mut b, 0xC0, 0x180);
        // segments at 0x140: text 8 bytes at 0x1A0
        put(&mut b, 0xC8, 0x140);
        put(&mut b, 0xCC, 1);
        put(&mut b, 0x140, 0x1A0);
        put(&mut b, 0x144, 8);
        put(&mut b, 0x148, 0);
        // named export "foo" at text+4
        put(&mut b, 0xD0, 0x14C);
        put(&mut b, 0xD4, 1);
        put(&mut b, 0x14C, 0x188);
        put(&mut b, 0x150, 4 << 4);
        // import relocations at 0x154: two, the second last
        put(&mut b, 0xF8, 0x154);
        put(&mut b, 0xFC, 2);
        put(&mut b, 0x154, 0);
        b[0x158] = R_ARM_ABS32;
        put(&mut b, 0x160, 4 << 4);
        b[0x164] = R_ARM_CALL;
        b[0x165] = 1;
        // named import "bar" -> list at 0x154
        put(&mut b, 0x100, 0x16C);
        put(&mut b, 0x104, 1);
        put(&mut b, 0x16C, 0x190);
        put(&mut b, 0x170, 0x154);
        // internal relocation at 0x174
        put(&mut b, 0x128, 0x174);
        put(&mut b, 0x12C, 1);
        put(&mut b, 0x174, 0);
        b[0x178] = R_ARM_ABS32;
        put(&mut b, 0x17C, 0x40);
        // empty tables still need in-file offsets
        for at in [0xD8, 0xE0, 0xE8, 0xF0, 0x108, 0x110, 0x118, 0x120, 0x130] {
            put(&mut b, at, 0x138);
        }
        b
    }

    #[test]
    fn parses_tables_and_import_lists() {
        let c = parse(&synthetic()).unwrap();
        assert_eq!((c.name.as_str(), c.is_crs), ("Test", false));
        assert_eq!(c.segments, [Segment { offset: 0x1A0, size: 8, kind: SegmentKind::Text }]);
        assert_eq!(c.named_exports, [NamedExport { name: "foo".into(), at: SegmentOffset { segment: 0, offset: 4 } }]);
        assert_eq!(c.imports.len(), 1);
        assert_eq!(c.imports[0].what, ImportRef::Named("bar".into()));
        assert_eq!(c.imports[0].relocations.iter().map(|r| r.kind).collect::<Vec<_>>(), [R_ARM_ABS32, R_ARM_CALL]);
        assert_eq!(c.internal[0].addend, 0x40);
    }

    #[test]
    fn rejects_what_points_outside() {
        let mut b = synthetic();
        b[0x150..0x154].copy_from_slice(&(9u32 << 4).to_le_bytes()); // export past the 8-byte text
        assert!(parse(&b).is_err());
        let mut b = synthetic();
        b[0x165] = 0; // the import list never ends
        assert!(parse(&b).is_err());
        let mut b = synthetic();
        b[0x80..0x84].copy_from_slice(b"FIXD");
        assert!(parse(&b).is_err());
    }
}

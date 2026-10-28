//! The RomFS: the title's read-only files, including its CRO modules.
//!
//! Layout (<https://www.3dbrew.org/wiki/RomFS>): an IVFC hash tree whose
//! level 3 is the filesystem. Level 3 starts at
//! align(0x60 + master hash size, level 3 block size) and begins with a
//! 0x28-byte header locating the directory and file metadata tables and the
//! file data. Directories and files are linked lists of table offsets;
//! 0xFFFFFFFF ends a list. Names are UTF-16LE.
//!
//! Only metadata is read here. Level 1-3 hashes are not checked (that means
//! reading the whole RomFS, ~3.5 GiB for these games); the superblock hash
//! over the IVFC header and master hash is checked by the caller.

use std::collections::HashSet;

use serde::Serialize;

use crate::source::{Le, Source, MAX_READ};
use crate::Error;

const IVFC_HEADER_SIZE: u64 = 0x60;
const LEVEL3_HEADER_SIZE: u32 = 0x28;
const NONE: u32 = 0xFFFF_FFFF;
const DIR_ENTRY_SIZE: usize = 0x18;
const FILE_ENTRY_SIZE: usize = 0x20;
/// A guard against cycles and absurd trees in malformed metadata.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Serialize)]
pub struct File {
    /// Path from the RomFS root, '/'-separated, no leading slash.
    pub path: String,
    /// Absolute offset in the dump.
    pub offset: u64,
    pub size: u64,
}

pub struct RomFs {
    pub files: Vec<File>,
    /// Where level 3 (the filesystem image a process gets through the
    /// SelfNCCH archive) lies in the dump.
    pub image: Image,
}

/// A range of the dump: absolute offset and size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Image {
    pub offset: u64,
    pub size: u64,
}

fn name(table: &Le, at: usize, header: usize, len: u32) -> Result<String, Error> {
    let raw = table.slice(at + header, len as usize)?;
    if !len.is_multiple_of(2) {
        return Err(Error::malformed("RomFS metadata", "odd-length UTF-16 name"));
    }
    let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let name = String::from_utf16(&units).map_err(|_| Error::malformed("RomFS metadata", "name is not valid UTF-16"))?;
    if name.contains('/') || name == "." || name == ".." {
        return Err(Error::malformed("RomFS metadata", format!("unsafe name {name:?}")));
    }
    Ok(name)
}

struct Walk<'a> {
    dirs: Le<'a>,
    files: Le<'a>,
    data: u64,
    data_end: u64,
    seen_dirs: HashSet<u32>,
    seen_files: HashSet<u32>,
    out: Vec<File>,
}

impl Walk<'_> {
    fn dir(&mut self, offset: u32, prefix: &str, depth: usize) -> Result<(), Error> {
        if depth > MAX_DEPTH {
            return Err(Error::malformed("RomFS metadata", "directories nest too deeply"));
        }
        if !self.seen_dirs.insert(offset) {
            return Err(Error::malformed("RomFS metadata", "a directory is linked twice"));
        }
        let at = offset as usize;
        let mut file = self.dirs.u32(at + 0x0C)?;
        while file != NONE {
            if !self.seen_files.insert(file) {
                return Err(Error::malformed("RomFS metadata", "a file is linked twice"));
            }
            let f = file as usize;
            let data_offset = self.files.u64(f + 0x08)?;
            let size = self.files.u64(f + 0x10)?;
            let leaf = name(&self.files, f, FILE_ENTRY_SIZE, self.files.u32(f + 0x1C)?)?;
            let offset = self.data.checked_add(data_offset).filter(|o| o.checked_add(size).is_some_and(|e| e <= self.data_end));
            let offset =
                offset.ok_or_else(|| Error::malformed("RomFS metadata", format!("{prefix}{leaf} lies outside the RomFS")))?;
            self.out.push(File { path: format!("{prefix}{leaf}"), offset, size });
            file = self.files.u32(f + 0x04)?;
        }
        let mut child = self.dirs.u32(at + 0x08)?;
        while child != NONE {
            let c = child as usize;
            let leaf = name(&self.dirs, c, DIR_ENTRY_SIZE, self.dirs.u32(c + 0x14)?)?;
            self.dir(child, &format!("{prefix}{leaf}/"), depth + 1)?;
            child = self.dirs.u32(c + 0x04)?;
        }
        Ok(())
    }
}

/// Lists the files of the RomFS of `size` bytes at `offset` in `source`.
pub fn read(source: &mut Source, offset: u64, size: u64) -> Result<RomFs, Error> {
    let ivfc = source.read_at("RomFS IVFC header", offset, IVFC_HEADER_SIZE)?;
    let h = Le::new("RomFS IVFC header", &ivfc);
    if h.slice(0, 4)? != b"IVFC" || h.u32(4)? != 0x10000 {
        return Err(Error::malformed("RomFS", "no IVFC header (encrypted, or not a RomFS)"));
    }
    let master_hash = u64::from(h.u32(0x08)?);
    let block_log2 = h.u32(0x4C)?;
    if !(9..=24).contains(&block_log2) {
        return Err(Error::malformed("RomFS", format!("level 3 block size 2^{block_log2}")));
    }
    let level3_rel = (IVFC_HEADER_SIZE + master_hash).next_multiple_of(1 << block_log2);
    let level3_size = h.u64(0x3C + 8)?;
    let rom_end = offset.checked_add(size).ok_or_else(|| Error::malformed("RomFS", "size overflows"))?;
    let level3 = offset + level3_rel;
    let level3_end = level3.checked_add(level3_size).filter(|&e| e <= rom_end);
    let level3_end = level3_end.ok_or_else(|| Error::malformed("RomFS", "level 3 lies outside the RomFS"))?;

    let header = source.read_at("RomFS level 3 header", level3, u64::from(LEVEL3_HEADER_SIZE))?;
    let l3 = Le::new("RomFS level 3 header", &header);
    if l3.u32(0)? != LEVEL3_HEADER_SIZE {
        return Err(Error::malformed("RomFS", "level 3 header has the wrong size"));
    }
    let table = |source: &mut Source, what, at: usize| -> Result<Vec<u8>, Error> {
        let start = level3 + u64::from(l3.u32(at)?);
        let len = u64::from(l3.u32(at + 4)?);
        if len > MAX_READ || start + len > level3_end {
            return Err(Error::malformed("RomFS", format!("{what} lies outside level 3")));
        }
        source.read_at(what, start, len)
    };
    let dirs = table(source, "RomFS directory table", 0x0C)?;
    let files = table(source, "RomFS file table", 0x1C)?;
    let data = level3 + u64::from(l3.u32(0x24)?);
    let mut walk = Walk {
        dirs: Le::new("RomFS directory table", &dirs),
        files: Le::new("RomFS file table", &files),
        data,
        data_end: level3_end,
        seen_dirs: HashSet::new(),
        seen_files: HashSet::new(),
        out: Vec::new(),
    };
    walk.dir(0, "", 0)?;
    walk.out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(RomFs { files: walk.out, image: Image { offset: level3, size: level3_size } })
}

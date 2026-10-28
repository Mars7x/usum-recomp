//! Synthetic dumps for tests. Every byte is made up here, in the shape the
//! format documents, with all stored SHA-256s correct. They contain no game
//! data, which is what lets the importer be tested in CI.

use sha2::{Digest, Sha256};

const MU: usize = 0x200;
const NONE: u32 = 0xFFFF_FFFF;
/// Where the game partition starts in a cartridge image (real cards also
/// put it at 0x4000).
const PARTITION_OFFSET: usize = 0x4000;
/// Text and rodata one page each, data 0x20 bytes: 0x2020 bytes of code.
pub const CODE_SIZE: usize = 0x2020;

#[derive(Debug, Clone)]
pub struct Spec {
    pub cartridge: bool,
    pub program_id: u64,
    pub product_code: &'static str,
    pub version: u16,
    /// Clears NoCrypto and scrambles everything after the NCCH header,
    /// the way an encrypted dump looks to a reader without keys.
    pub encrypted: bool,
    /// File names under `cro/`, each given a small CRO-shaped body.
    pub modules: Vec<&'static str>,
}

impl Default for Spec {
    fn default() -> Spec {
        Spec {
            cartridge: true,
            program_id: 0x0004_0000_00FF_F000,
            product_code: "CTR-P-TEST",
            version: 0,
            encrypted: false,
            modules: vec!["Alpha.cro", "Beta.cro"],
        }
    }
}

/// The synthetic code: a deterministic pattern, not instructions.
pub fn code() -> Vec<u8> {
    (0..CODE_SIZE).map(|i| (i as u8).wrapping_mul(31).wrapping_add(7)).collect()
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn align(n: usize, to: usize) -> usize {
    n.next_multiple_of(to)
}

fn utf16(name: &str) -> Vec<u8> {
    name.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn exheader(spec: &Spec) -> Vec<u8> {
    let mut b = vec![0u8; 0x800];
    b[..4].copy_from_slice(b"fixt");
    for (at, address, size) in [(0x10, 0x0010_0000, 0x1000), (0x20, 0x0010_1000, 0x1000), (0x30, 0x0010_2000, 0x20)] {
        put32(&mut b, at, address);
        put32(&mut b, at + 4, 1);
        put32(&mut b, at + 8, size);
    }
    put32(&mut b, 0x1C, 0x4000); // stack size
    put64(&mut b, 0x200, spec.program_id);
    b[0x250..0x258].copy_from_slice(b"fs:USER\0");
    b[0x258..0x260].copy_from_slice(b"gsp::Gpu");
    b
}

fn exefs(code: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; align(MU + code.len(), MU)];
    b[..5].copy_from_slice(b".code");
    put32(&mut b, 12, code.len() as u32);
    let hash = Sha256::digest(code);
    b[0x1E0..0x200].copy_from_slice(&hash);
    b[MU..MU + code.len()].copy_from_slice(code);
    b
}

/// A CRO-shaped body: a hash table, then the magic, then a recognizable
/// fill derived from the name.
fn module_body(name: &str) -> Vec<u8> {
    let mut b = vec![0u8; 0x100];
    b[0x80..0x84].copy_from_slice(b"CRO0");
    for (i, byte) in b[0x84..].iter_mut().enumerate() {
        *byte = name.as_bytes()[i % name.len()];
    }
    b
}

fn romfs(modules: &[&str]) -> Vec<u8> {
    const LEVEL3: usize = 0x1000;
    // directories: root, then "cro"
    let mut dirs = vec![0u8; 0x18];
    put32(&mut dirs, 0x04, NONE);
    put32(&mut dirs, 0x08, 0x18);
    put32(&mut dirs, 0x0C, NONE);
    put32(&mut dirs, 0x10, NONE);
    let name = utf16("cro");
    let mut cro = vec![0u8; 0x18 + align(name.len(), 4)];
    put32(&mut cro, 0x04, NONE);
    put32(&mut cro, 0x08, NONE);
    put32(&mut cro, 0x0C, if modules.is_empty() { NONE } else { 0 });
    put32(&mut cro, 0x10, NONE);
    put32(&mut cro, 0x14, name.len() as u32);
    cro[0x18..0x18 + name.len()].copy_from_slice(&name);
    dirs.extend(cro);

    let mut files = Vec::new();
    let mut data = Vec::new();
    for (i, module) in modules.iter().enumerate() {
        let name = utf16(module);
        let entry_len = 0x20 + align(name.len(), 4);
        let mut entry = vec![0u8; entry_len];
        put32(&mut entry, 0x00, 0x18);
        let next = files.len() + entry_len;
        put32(&mut entry, 0x04, if i + 1 == modules.len() { NONE } else { next as u32 });
        let body = module_body(module);
        put64(&mut entry, 0x08, data.len() as u64);
        put64(&mut entry, 0x10, body.len() as u64);
        put32(&mut entry, 0x18, NONE);
        put32(&mut entry, 0x1C, name.len() as u32);
        entry[0x20..0x20 + name.len()].copy_from_slice(&name);
        files.extend(entry);
        data.extend(body);
        data.resize(align(data.len(), 16), 0);
    }

    let dir_meta = 0x28 + 4;
    let file_hash = dir_meta + dirs.len();
    let file_meta = file_hash + 4;
    let file_data = align(file_meta + files.len(), 16);
    let mut level3 = vec![0u8; file_data];
    put32(&mut level3, 0x00, 0x28);
    for (at, offset, len) in [(0x04, 0x28, 4), (0x0C, dir_meta, dirs.len()), (0x14, file_hash, 4), (0x1C, file_meta, files.len())]
    {
        put32(&mut level3, at, offset as u32);
        put32(&mut level3, at + 4, len as u32);
    }
    put32(&mut level3, 0x24, file_data as u32);
    put32(&mut level3, 0x28, NONE);
    level3[dir_meta..dir_meta + dirs.len()].copy_from_slice(&dirs);
    put32(&mut level3, file_hash, NONE);
    level3[file_meta..file_meta + files.len()].copy_from_slice(&files);
    level3.extend(data);

    let mut romfs = vec![0u8; align(LEVEL3 + level3.len(), MU)];
    romfs[..4].copy_from_slice(b"IVFC");
    put32(&mut romfs, 0x04, 0x10000);
    put32(&mut romfs, 0x08, 0x20); // master hash size
    put64(&mut romfs, 0x3C + 8, level3.len() as u64);
    put32(&mut romfs, 0x3C + 0x10, 12); // level 3 block size 2^12
    romfs[LEVEL3..LEVEL3 + level3.len()].copy_from_slice(&level3);
    romfs
}

/// Builds the image `spec` describes.
pub fn build(spec: &Spec) -> Vec<u8> {
    let exheader = exheader(spec);
    let exefs = exefs(&code());
    let romfs = romfs(&spec.modules);
    let exefs_at = MU + exheader.len();
    let romfs_at = exefs_at + exefs.len();
    let size = romfs_at + romfs.len();

    let mut part = vec![0u8; size];
    part[0x100..0x104].copy_from_slice(b"NCCH");
    put32(&mut part, 0x104, (size / MU) as u32);
    put64(&mut part, 0x108, spec.program_id);
    part[0x110..0x112].copy_from_slice(b"00");
    part[0x112..0x114].copy_from_slice(&spec.version.to_le_bytes());
    put64(&mut part, 0x118, spec.program_id);
    let code = spec.product_code.as_bytes();
    part[0x150..0x150 + code.len()].copy_from_slice(code);
    part[0x160..0x180].copy_from_slice(&Sha256::digest(&exheader[..0x400]));
    put32(&mut part, 0x180, 0x400);
    part[0x188 + 5] = 0x03;
    part[0x188 + 7] = if spec.encrypted { 0 } else { 0x04 };
    for (at, value) in [
        (0x1A0, exefs_at / MU),
        (0x1A4, exefs.len() / MU),
        (0x1A8, 1),
        (0x1B0, romfs_at / MU),
        (0x1B4, romfs.len() / MU),
        (0x1B8, 1),
    ] {
        put32(&mut part, at, value as u32);
    }
    part[0x1C0..0x1E0].copy_from_slice(&Sha256::digest(&exefs[..MU]));
    part[0x1E0..0x200].copy_from_slice(&Sha256::digest(&romfs[..MU]));
    part[MU..exefs_at].copy_from_slice(&exheader);
    part[exefs_at..romfs_at].copy_from_slice(&exefs);
    part[romfs_at..].copy_from_slice(&romfs);
    if spec.encrypted {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for byte in &mut part[MU..] {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *byte ^= (state >> 56) as u8;
        }
    }
    if !spec.cartridge {
        return part;
    }
    let mut image = vec![0u8; PARTITION_OFFSET];
    image[0x100..0x104].copy_from_slice(b"NCSD");
    put32(&mut image, 0x104, ((PARTITION_OFFSET + size) / MU) as u32);
    put64(&mut image, 0x108, spec.program_id);
    put32(&mut image, 0x120, (PARTITION_OFFSET / MU) as u32);
    put32(&mut image, 0x124, (size / MU) as u32);
    image.extend(part);
    image
}

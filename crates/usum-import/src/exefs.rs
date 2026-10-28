//! The ExeFS: a flat archive of up to 10 files (`.code`, `icon`, `banner`,
//! `logo`), each with a SHA-256 in the header.
//!
//! Layout (<https://www.3dbrew.org/wiki/ExeFS>): 10 entries of name[8],
//! offset u32, size u32 at 0x00; 10 hashes at 0xC0, stored in reverse, so
//! entry i's hash is at 0x200 - 0x20 * (i + 1). File offsets count from the
//! end of the 0x200-byte header.

use serde::Serialize;

use crate::source::Le;
use crate::Error;

pub const HEADER_SIZE: u64 = 0x200;
const ENTRIES: usize = 10;

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    /// Offset from the start of the ExeFS (header included).
    pub offset: u64,
    pub size: u64,
    #[serde(skip)]
    pub sha256: [u8; 32],
}

pub fn parse(header: &[u8], exefs_size: u64) -> Result<Vec<Entry>, Error> {
    let h = Le::new("ExeFS header", header);
    let mut entries: Vec<Entry> = Vec::new();
    for i in 0..ENTRIES {
        let raw_name = h.slice(i * 16, 8)?;
        if raw_name.iter().all(|&b| b == 0) {
            continue;
        }
        // ciphertext shows up here as non-ASCII names
        let name = h.ascii(i * 16, 8).map_err(|_| Error::Encrypted)?;
        let offset = HEADER_SIZE + u64::from(h.u32(i * 16 + 8)?);
        let size = u64::from(h.u32(i * 16 + 12)?);
        if offset + size > exefs_size {
            return Err(Error::malformed("ExeFS header", format!("{name} extends past the ExeFS")));
        }
        if entries.iter().any(|e| e.name == name) {
            return Err(Error::malformed("ExeFS header", format!("{name} appears twice")));
        }
        entries.push(Entry { name, offset, size, sha256: h.hash(0x200 - 0x20 * (i + 1))? });
    }
    if entries.is_empty() {
        return Err(Error::malformed("ExeFS header", "no files"));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_and_reversed_hashes() {
        let mut h = vec![0u8; 0x200];
        h[..5].copy_from_slice(b".code");
        h[12..16].copy_from_slice(&0x10u32.to_le_bytes());
        h[16..20].copy_from_slice(b"icon");
        h[24..28].copy_from_slice(&0x10u32.to_le_bytes());
        h[28..32].copy_from_slice(&0x20u32.to_le_bytes());
        h[0x1E0] = 0xAA; // entry 0's hash
        h[0x1C0] = 0xBB; // entry 1's hash
        let e = parse(&h, 0x300).unwrap();
        assert_eq!((e[0].name.as_str(), e[0].offset, e[0].size, e[0].sha256[0]), (".code", 0x200, 0x10, 0xAA));
        assert_eq!((e[1].name.as_str(), e[1].offset, e[1].sha256[0]), ("icon", 0x210, 0xBB));
        assert!(parse(&h, 0x220).is_err(), "icon past the end");
    }

    #[test]
    fn ciphertext_names_mean_encrypted() {
        let mut h = vec![0u8; 0x200];
        h[..8].copy_from_slice(&[0x9C, 0x11, 0xF3, 0x05, 0x7E, 0xD0, 0x42, 0x88]);
        assert!(matches!(parse(&h, 0x1000), Err(Error::Encrypted)));
    }
}

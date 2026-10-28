//! Import and inspection of user-supplied dumps.
//!
//! This is the first code that touches a user's file. It decides whether
//! the file is a supported, complete, decrypted 3DS game executable,
//! checks every SHA-256 the format stores for the parts it reads, extracts
//! the executable code and the list of CRO modules, and matches the result
//! against explicitly recorded version profiles. It never writes anything
//! and never decides from a file name.
//!
//! The input contract is documented in docs/INPUT.md.

pub mod blz;
pub mod cro;
mod error;
pub mod exefs;
pub mod exheader;
pub mod fixture;
pub mod ncch;
pub mod profile;
pub mod romfs;
pub mod source;

use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

pub use error::Error;
pub use profile::{Game, Profile};
use source::{Source, MAX_READ};

/// Title ID high words (<https://www.3dbrew.org/wiki/Titles>).
const TID_HIGH_APPLICATION: u32 = 0x0004_0000;
const TID_HIGH_UPDATE: u32 = 0x0004_000E;
const TID_HIGH_DLC: u32 = 0x0004_008C;
/// A .cia begins with its header size, always 0x2020 (<https://www.3dbrew.org/wiki/CIA>).
const CIA_HEADER_SIZE: u32 = 0x2020;
/// A CRO's magic sits after its 0x80-byte hash table (<https://www.3dbrew.org/wiki/CRO0>).
const CRO_MAGIC_OFFSET: usize = 0x80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// A cartridge image (.3ds / .cci): the game is partition 0.
    Ncsd,
    /// A bare executable partition (.cxi).
    Ncch,
}

#[derive(Debug, Clone, Serialize)]
pub struct Code {
    /// Size stored in the ExeFS.
    pub stored_size: u64,
    pub size: usize,
    pub sha256: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

/// A file in the RomFS that holds code: a CRO module or the static.crs.
#[derive(Debug, Clone, Serialize)]
pub struct Module {
    pub path: String,
    /// Where the file lies in the dump (absolute offset).
    pub offset: u64,
    pub size: u64,
    pub sha256: String,
    pub cro_magic: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    pub format: Format,
    pub file_size: u64,
    pub ncsd: Option<ncch::Ncsd>,
    pub partition_offset: u64,
    pub ncch: ncch::Ncch,
    pub exheader: exheader::ExHeader,
    pub exefs: Vec<exefs::Entry>,
    pub code: Code,
    pub romfs_files: usize,
    /// The RomFS level 3 image in the dump, if there is a RomFS.
    pub romfs_image: Option<romfs::Image>,
    /// Every RomFS file (metadata only), for tools; not in the JSON report.
    #[serde(skip)]
    pub romfs_listing: Vec<romfs::File>,
    pub modules: Vec<Module>,
    /// Things that are odd but not fatal.
    pub notes: Vec<String>,
    pub profile: profile::Match,
}

impl Inspection {
    pub fn program_id(&self) -> String {
        format!("{:016X}", self.ncch.program_id)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Locates the executable partition: returns the format, the NCSD header
/// if any, and the partition's offset.
fn locate(source: &mut Source) -> Result<(Format, Option<ncch::Ncsd>, u64), Error> {
    if source.len() < ncch::HEADER_SIZE {
        return Err(Error::NotADump("the file is smaller than any 3DS header"));
    }
    let start = source.read_at("first header", 0, ncch::HEADER_SIZE)?;
    match &start[0x100..0x104] {
        b"NCSD" => {
            let ncsd = ncch::parse_ncsd(&start)?;
            let game = ncsd.partitions[0];
            if game.size == 0 {
                return Err(Error::malformed("NCSD header", "partition 0 (the game) is empty"));
            }
            if game.end()? > source.len() {
                return Err(Error::Truncated { what: "the game partition", needed_end: game.end()?, file_size: source.len() });
            }
            Ok((Format::Ncsd, Some(ncsd), game.offset))
        }
        b"NCCH" => Ok((Format::Ncch, None, 0)),
        _ if u32::from_le_bytes(start[..4].try_into().expect("4 bytes")) == CIA_HEADER_SIZE => Err(Error::UnsupportedFormat(
            "this is a .cia; only decrypted cartridge images (.3ds/.cci) and executables (.cxi) are supported for now",
        )),
        _ => Err(Error::NotADump("not a 3DS cartridge image (.3ds/.cci) or executable (.cxi)")),
    }
}

fn check_title_kind(program_id: u64) -> Result<(), Error> {
    match (program_id >> 32) as u32 {
        TID_HIGH_UPDATE => Err(Error::IsUpdate(program_id)),
        TID_HIGH_DLC => Err(Error::IsDlc(program_id)),
        _ => Ok(()),
    }
}

/// Validates and inspects the dump at `path`.
pub fn inspect(path: &Path) -> Result<Inspection, Error> {
    let mut source = Source::open(path)?;
    let mut notes = Vec::new();
    let (format, ncsd, base) = locate(&mut source)?;

    let header = source.read_at("NCCH header", base, ncch::HEADER_SIZE)?;
    let ncch = ncch::parse_ncch(&header)?;
    check_title_kind(ncch.program_id)?;
    if !ncch.executable || ncch.exheader_size == 0 {
        return Err(Error::NotExecutable);
    }
    let content_end = base.checked_add(ncch.content_size).ok_or_else(|| Error::malformed("NCCH header", "size overflows"))?;
    if content_end > source.len() {
        return Err(Error::Truncated { what: "the NCCH partition", needed_end: content_end, file_size: source.len() });
    }
    if (ncch.program_id >> 32) as u32 != TID_HIGH_APPLICATION {
        notes.push(format!("program ID {:016X} is not an ordinary application", ncch.program_id));
    }

    // The exheader hash is computed over plaintext, so it is the test for
    // decryption: flags can be wrong, a matching SHA-256 cannot.
    let raw_exheader = source.read_at("extended header", base + ncch::HEADER_SIZE, exheader::SIZE as u64)?;
    if sha256(&raw_exheader) != ncch.exheader_hash {
        return Err(if ncch.no_crypto { Error::HashMismatch { what: "the extended header".into() } } else { Error::Encrypted });
    }
    if !ncch.no_crypto {
        notes.push("the NoCrypto flag is clear, but the contents hash as plaintext".into());
    }
    let exheader = exheader::parse(&raw_exheader)?;
    if exheader.aci_program_id != ncch.program_id {
        notes.push(format!("access control is for {:016X}, not this title", exheader.aci_program_id));
    }

    // ExeFS: the superblock hash covers its header, which holds the files'
    // hashes; each file read is checked against its own.
    let exefs_base = base + ncch.exefs.offset;
    if ncch.exefs.size < exefs::HEADER_SIZE
        || ncch.exefs_hash_region < exefs::HEADER_SIZE
        || ncch.exefs_hash_region > ncch.exefs.size
    {
        return Err(Error::malformed("NCCH header", "ExeFS bounds"));
    }
    let hashed = source.read_at("ExeFS", exefs_base, ncch.exefs_hash_region)?;
    if sha256(&hashed) != ncch.exefs_hash {
        return Err(Error::HashMismatch { what: "the ExeFS header".into() });
    }
    let exefs = exefs::parse(&hashed[..exefs::HEADER_SIZE as usize], ncch.exefs.size)?;
    let code_entry = exefs.iter().find(|e| e.name == ".code").ok_or_else(|| Error::malformed("ExeFS", "no .code"))?;
    let stored = source.read_at("ExeFS .code", exefs_base + code_entry.offset, code_entry.size)?;
    if sha256(&stored) != code_entry.sha256 {
        return Err(Error::HashMismatch { what: "the ExeFS .code".into() });
    }
    let bytes = if exheader.compressed_code { blz::decompress(&stored, MAX_READ as usize)? } else { stored };
    let (min, max) = exheader.code_size_range();
    if !(min..=max).contains(&(bytes.len() as u64)) {
        return Err(Error::malformed("ExeFS .code", format!("{} bytes, but the exheader lays out {min}..={max}", bytes.len())));
    }
    let code = Code { stored_size: code_entry.size, size: bytes.len(), sha256: sha256_hex(&bytes), bytes };

    // RomFS: the superblock hash covers the IVFC header and master hash.
    let mut romfs_files = 0;
    let mut romfs_image = None;
    let mut romfs_listing = Vec::new();
    let mut modules = Vec::new();
    if let Some(region) = ncch.romfs {
        let romfs_base = base + region.offset;
        if ncch.romfs_hash_region == 0 || ncch.romfs_hash_region > region.size {
            return Err(Error::malformed("NCCH header", "RomFS hash region"));
        }
        let hashed = source.read_at("RomFS superblock", romfs_base, ncch.romfs_hash_region)?;
        if sha256(&hashed) != ncch.romfs_hash {
            return Err(Error::HashMismatch { what: "the RomFS superblock".into() });
        }
        let romfs = romfs::read(&mut source, romfs_base, region.size)?;
        romfs_files = romfs.files.len();
        romfs_image = Some(romfs.image);
        romfs_listing = romfs.files.clone();
        for file in romfs.files.iter().filter(|f| f.path.ends_with(".cro") || f.path.ends_with(".crs")) {
            let data = source.read_at("RomFS module", file.offset, file.size)?;
            let cro_magic = data.get(CRO_MAGIC_OFFSET..CRO_MAGIC_OFFSET + 4) == Some(b"CRO0");
            if !cro_magic {
                notes.push(format!("{} has no CRO0 magic", file.path));
            }
            modules.push(Module {
                path: file.path.clone(),
                offset: file.offset,
                size: file.size,
                sha256: sha256_hex(&data),
                cro_magic,
            });
        }
    } else {
        notes.push("the partition has no RomFS".into());
    }

    let profile = profile::find(ncch.program_id, &code.sha256);
    Ok(Inspection {
        format,
        file_size: source.len(),
        ncsd,
        partition_offset: base,
        ncch,
        exheader,
        exefs,
        code,
        romfs_files,
        romfs_image,
        romfs_listing,
        modules,
        notes,
        profile,
    })
}

#[cfg(test)]
mod testutil;

#[cfg(test)]
mod tests {
    use super::*;
    use fixture::Spec;
    use testutil::write_temp;

    fn inspect_bytes(name: &str, bytes: &[u8]) -> Result<Inspection, Error> {
        inspect(&write_temp(name, bytes))
    }

    #[test]
    fn sha256_matches_fips_vector() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn inspects_synthetic_cartridge() {
        let i = inspect_bytes("cart.3ds", &fixture::build(&Spec::default())).unwrap();
        assert_eq!(i.format, Format::Ncsd);
        assert_eq!(i.partition_offset, 0x4000);
        assert_eq!(i.program_id(), "0004000000FFF000");
        assert_eq!(i.ncch.product_code, "CTR-P-TEST");
        assert_eq!(i.exheader.services, ["fs:USER", "gsp::Gpu"]);
        assert_eq!(i.code.bytes, fixture::code());
        assert_eq!(i.code.sha256, sha256_hex(&fixture::code()));
        let paths: Vec<_> = i.modules.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, ["cro/Alpha.cro", "cro/Beta.cro"]);
        assert!(i.modules.iter().all(|m| m.cro_magic));
        assert_eq!(i.profile, profile::Match::OtherTitle);
        assert!(i.notes.is_empty(), "{:?}", i.notes);
    }

    #[test]
    fn inspects_bare_executable() {
        let spec = Spec { cartridge: false, ..Spec::default() };
        let i = inspect_bytes("bare.cxi", &fixture::build(&spec)).unwrap();
        assert_eq!((i.format, i.partition_offset), (Format::Ncch, 0));
    }

    #[test]
    fn a_known_title_id_with_unrecorded_code_is_not_a_profile() {
        let spec = Spec { program_id: 0x0004_0000_001B_5000, ..Spec::default() };
        let i = inspect_bytes("fake-us.3ds", &fixture::build(&spec)).unwrap();
        assert_eq!(i.profile, profile::Match::UnknownRevision { game: Game::UltraSun });
    }

    #[test]
    fn file_name_is_irrelevant() {
        let bytes = fixture::build(&Spec::default());
        let a = inspect_bytes("Pokemon Ultra Sun (USA).3ds", &bytes).unwrap();
        assert_eq!(a.profile, profile::Match::OtherTitle);
    }

    #[test]
    fn encrypted_input_is_reported_as_encrypted() {
        let spec = Spec { encrypted: true, ..Spec::default() };
        assert!(matches!(inspect_bytes("enc.3ds", &fixture::build(&spec)), Err(Error::Encrypted)));
    }

    #[test]
    fn updates_and_dlc_are_refused() {
        let spec = Spec { program_id: 0x0004_000E_001B_5000, ..Spec::default() };
        assert!(matches!(inspect_bytes("upd.3ds", &fixture::build(&spec)), Err(Error::IsUpdate(_))));
        let spec = Spec { program_id: 0x0004_008C_001B_5000, ..Spec::default() };
        assert!(matches!(inspect_bytes("dlc.3ds", &fixture::build(&spec)), Err(Error::IsDlc(_))));
    }

    #[test]
    fn truncation_is_reported_at_every_length() {
        let bytes = fixture::build(&Spec::default());
        for cut in (0..bytes.len()).step_by(0x100) {
            match inspect_bytes("cut.3ds", &bytes[..cut]) {
                Err(Error::Truncated { .. } | Error::NotADump(_)) => {}
                other => panic!("cut at {cut:#x}: {:?}", other.map(|_| ())),
            }
        }
    }

    #[test]
    fn corruption_is_caught_by_hashes() {
        let clean = fixture::build(&Spec::default());
        // one byte in the code, the exheader, and the RomFS superblock
        for (at, what) in
            [(0x4000 + 0xA00 + 0x200 + 5, "code"), (0x4000 + 0x200 + 1, "exheader"), (0x4000 + 0x2E00 + 0x20, "romfs")]
        {
            let mut bytes = clean.clone();
            bytes[at] ^= 0x40;
            assert!(matches!(inspect_bytes("corrupt.3ds", &bytes), Err(Error::HashMismatch { .. })), "{what}");
        }
    }

    #[test]
    fn cia_and_garbage_get_specific_errors() {
        let mut cia = vec![0u8; 0x400];
        cia[..4].copy_from_slice(&0x2020u32.to_le_bytes());
        assert!(matches!(inspect_bytes("x.cia", &cia), Err(Error::UnsupportedFormat(_))));
        assert!(matches!(inspect_bytes("x.bin", &[0x5A; 0x400]), Err(Error::NotADump(_))));
        assert!(matches!(inspect_bytes("empty.3ds", &[]), Err(Error::NotADump(_))));
    }

    /// Deterministic mutation testing: random bytes in the headers and
    /// tables must produce an error or a result, never a panic, hang or
    /// huge allocation (reads are capped by MAX_READ).
    #[test]
    fn mutated_inputs_never_panic() {
        let clean = fixture::build(&Spec::default());
        let regions = [0x100..0x200, 0x4100..0x4200, 0x4200..0x4600, 0x4A00..0x4C00, 0x6E00..0x6E60, 0x7E00..0x7F40];
        let mut state = 0x853C_49E6_748F_EA9Bu64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let path = write_temp("mutant.3ds", &clean);
        for round in 0..3000 {
            let mut bytes = clean.clone();
            let region = &regions[round % regions.len()];
            for _ in 0..1 + next() % 4 {
                let at = region.start + (next() as usize) % region.len();
                bytes[at] = next() as u8;
            }
            std::fs::write(&path, &bytes).unwrap();
            let _ = inspect(&path);
        }
    }
}

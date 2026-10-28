//! NCSD (cartridge image) and NCCH (partition) headers.
//!
//! Layouts: <https://www.3dbrew.org/wiki/NCSD>, <https://www.3dbrew.org/wiki/NCCH>.
//! Both headers are 0x200 bytes, a 0x100-byte RSA signature first. Offsets
//! and sizes are in media units of `0x200 << flags[6]`.

use serde::Serialize;

use crate::source::Le;
use crate::Error;

pub const HEADER_SIZE: u64 = 0x200;
const MEDIA_UNIT_BASE: u64 = 0x200;
/// Partition table entries in an NCSD.
pub const NCSD_PARTITIONS: usize = 8;

/// flags[5]: content type bits; bit 1 marks an executable (CXI).
const FLAG5_EXECUTABLE: u8 = 0x02;
/// flags[7] bits.
const FLAG7_NO_ROMFS: u8 = 0x02;
const FLAG7_NO_CRYPTO: u8 = 0x04;

/// What 3dbrew documents the NCSD partitions of a cartridge to hold.
pub fn partition_role(index: usize) -> &'static str {
    match index {
        0 => "game executable",
        1 => "electronic manual",
        2 => "Download Play child",
        6 => "New 3DS update data",
        7 => "update data",
        _ => "unused",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Region {
    pub offset: u64,
    pub size: u64,
}

impl Region {
    pub fn end(&self) -> Result<u64, Error> {
        self.offset.checked_add(self.size).ok_or_else(|| Error::malformed("region", "end overflows"))
    }
}

/// The NCSD header of a cartridge image.
#[derive(Debug, Clone, Serialize)]
pub struct Ncsd {
    pub media_id: u64,
    /// The image size the header declares. Trimmed dumps are smaller.
    pub image_size: u64,
    /// Byte ranges of the eight partitions; absent ones have size 0.
    pub partitions: [Region; NCSD_PARTITIONS],
}

pub fn parse_ncsd(header: &[u8]) -> Result<Ncsd, Error> {
    let h = Le::new("NCSD header", header);
    if h.slice(0x100, 4)? != b"NCSD" {
        return Err(Error::NotADump("no NCSD magic"));
    }
    let unit = media_unit(h.u8(0x188 + 6)?)?;
    let mut partitions = [Region { offset: 0, size: 0 }; NCSD_PARTITIONS];
    for (i, p) in partitions.iter_mut().enumerate() {
        let offset = u64::from(h.u32(0x120 + i * 8)?) * unit;
        let size = u64::from(h.u32(0x124 + i * 8)?) * unit;
        *p = Region { offset, size };
    }
    Ok(Ncsd { media_id: h.u64(0x108)?, image_size: u64::from(h.u32(0x104)?) * unit, partitions })
}

fn media_unit(exponent: u8) -> Result<u64, Error> {
    // retail content uses 0; anything that would overflow is nonsense
    if exponent > 8 {
        return Err(Error::malformed("header", format!("media unit exponent {exponent}")));
    }
    Ok(MEDIA_UNIT_BASE << exponent)
}

/// The fields of an NCCH header this project uses. Offsets are relative to
/// the start of the partition.
#[derive(Debug, Clone, Serialize)]
pub struct Ncch {
    pub content_size: u64,
    pub partition_id: u64,
    pub maker_code: String,
    pub version: u16,
    pub program_id: u64,
    pub product_code: String,
    pub exheader_size: u32,
    #[serde(skip)]
    pub exheader_hash: [u8; 32],
    pub crypto_method: u8,
    pub executable: bool,
    pub no_crypto: bool,
    pub exefs: Region,
    pub exefs_hash_region: u64,
    #[serde(skip)]
    pub exefs_hash: [u8; 32],
    /// None when the partition has no RomFS (or says not to mount it).
    pub romfs: Option<Region>,
    pub romfs_hash_region: u64,
    #[serde(skip)]
    pub romfs_hash: [u8; 32],
}

pub fn parse_ncch(header: &[u8]) -> Result<Ncch, Error> {
    let h = Le::new("NCCH header", header);
    if h.slice(0x100, 4)? != b"NCCH" {
        return Err(Error::malformed("NCCH header", "no NCCH magic where the partition starts"));
    }
    let flags = h.slice(0x188, 8)?;
    let unit = media_unit(flags[6])?;
    let mu = |at: usize| -> Result<u64, Error> { Ok(u64::from(h.u32(at)?) * unit) };
    let romfs = Region { offset: mu(0x1B0)?, size: mu(0x1B4)? };
    Ok(Ncch {
        content_size: mu(0x104)?,
        partition_id: h.u64(0x108)?,
        maker_code: h.ascii(0x110, 2)?,
        version: h.u16(0x112)?,
        program_id: h.u64(0x118)?,
        product_code: h.ascii(0x150, 0x10)?,
        exheader_size: h.u32(0x180)?,
        exheader_hash: h.hash(0x160)?,
        crypto_method: flags[3],
        executable: flags[5] & FLAG5_EXECUTABLE != 0,
        no_crypto: flags[7] & FLAG7_NO_CRYPTO != 0,
        exefs: Region { offset: mu(0x1A0)?, size: mu(0x1A4)? },
        exefs_hash_region: mu(0x1A8)?,
        exefs_hash: h.hash(0x1C0)?,
        romfs: (romfs.size != 0 && flags[7] & FLAG7_NO_ROMFS == 0).then_some(romfs),
        romfs_hash_region: mu(0x1B8)?,
        romfs_hash: h.hash(0x1E0)?,
    })
}

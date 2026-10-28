//! The extended header: where the code is loaded, what the title may call.
//!
//! Layout: <https://www.3dbrew.org/wiki/NCCH/Extended_Header>. Only the
//! first 0x400 bytes (system control info and access control info) are
//! read; the NCCH header's exheader hash covers exactly those.

use serde::Serialize;

use crate::source::Le;
use crate::Error;

pub const SIZE: usize = 0x400;
pub const PAGE_SIZE: u32 = 0x1000;
const SERVICE_SLOTS: usize = 34;
const SERVICE_NAME_LEN: usize = 8;
const DEPENDENCY_SLOTS: usize = 48;
/// ARM11 kernel capability descriptors (3dbrew "NCCH/Extended Header").
const KERNEL_CAPS: usize = 28;

/// One segment of the executable: its load address, the pages reserved for
/// it, and the bytes it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CodeSet {
    pub address: u32,
    pub pages: u32,
    pub size: u32,
}

/// An address range the exheader lets the process map (`mappings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Mapping {
    pub start: u32,
    /// Exclusive.
    pub end: u32,
    pub read_only: bool,
    /// Static (cacheable) memory rather than IO.
    pub cacheable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExHeader {
    pub process_name: String,
    pub compressed_code: bool,
    pub sd_application: bool,
    pub remaster_version: u16,
    pub text: CodeSet,
    pub stack_size: u32,
    pub rodata: CodeSet,
    pub data: CodeSet,
    pub bss_size: u32,
    /// Program IDs of the system modules the title depends on.
    pub dependencies: Vec<String>,
    pub save_data_size: u64,
    pub jump_id: u64,
    /// Program ID in the access control info, normally the title's own.
    pub aci_program_id: u64,
    pub ideal_processor: u8,
    /// The Old 3DS system mode the title requests (ACI flag0 bits 4-7):
    /// 0 is Prod (64 MB for applications), 2-5 other APPMEMTYPEs.
    pub old3ds_system_mode: u8,
    /// The New 3DS system mode (ACI flag2 bits 0-3).
    pub new3ds_system_mode: u8,
    pub priority: u8,
    pub extdata_id: u64,
    /// The services the title is allowed to connect to: what an HLE
    /// runtime must at least be ready for.
    pub services: Vec<String>,
    /// The ARM11 kernel capability descriptors (0x370, 28 words), raw.
    pub kernel_caps: Vec<u32>,
    /// The kernel release version descriptor (prefix 0b1111110), as
    /// (major, minor). It selects the process's memory layout.
    pub kernel_version: Option<(u8, u8)>,
}

fn codeset(h: &Le, at: usize) -> Result<CodeSet, Error> {
    Ok(CodeSet { address: h.u32(at)?, pages: h.u32(at + 4)?, size: h.u32(at + 8)? })
}

pub fn parse(bytes: &[u8]) -> Result<ExHeader, Error> {
    let h = Le::new("extended header", bytes);
    let flags = h.u8(0x0D)?;
    let mut services = Vec::new();
    for slot in 0..SERVICE_SLOTS {
        let name = h.ascii(0x250 + slot * SERVICE_NAME_LEN, SERVICE_NAME_LEN)?;
        if !name.is_empty() {
            services.push(name);
        }
    }
    let mut dependencies = Vec::new();
    for slot in 0..DEPENDENCY_SLOTS {
        let id = h.u64(0x40 + slot * 8)?;
        if id != 0 {
            dependencies.push(format!("{id:016X}"));
        }
    }
    let flag0 = h.u8(0x20E)?;
    let kernel_caps: Vec<u32> = (0..KERNEL_CAPS).map(|i| h.u32(0x370 + 4 * i)).collect::<Result<_, _>>()?;
    let kernel_version = kernel_caps.iter().find(|&&d| d & 0xFE00_0000 == 0xFC00_0000).map(|&d| ((d >> 8) as u8, d as u8));
    let exheader = ExHeader {
        process_name: h.ascii(0x0, 8)?,
        compressed_code: flags & 0x01 != 0,
        sd_application: flags & 0x02 != 0,
        remaster_version: h.u16(0x0E)?,
        text: codeset(&h, 0x10)?,
        stack_size: h.u32(0x1C)?,
        rodata: codeset(&h, 0x20)?,
        data: codeset(&h, 0x30)?,
        bss_size: h.u32(0x3C)?,
        dependencies,
        save_data_size: h.u64(0x1C0)?,
        jump_id: h.u64(0x1C8)?,
        aci_program_id: h.u64(0x200)?,
        ideal_processor: flag0 & 0x03,
        old3ds_system_mode: flag0 >> 4,
        new3ds_system_mode: h.u8(0x20D)? & 0x0F,
        priority: h.u8(0x20F)?,
        extdata_id: h.u64(0x230)?,
        services,
        kernel_caps,
        kernel_version,
    };
    exheader.validate()?;
    Ok(exheader)
}

impl ExHeader {
    /// Checks that the segments are page-aligned, ordered, and fit their
    /// reserved pages. A loader would otherwise map overlapping memory.
    fn validate(&self) -> Result<(), Error> {
        let bad = |detail: String| Err(Error::malformed("extended header", detail));
        let mut previous_end = 0u64;
        for (name, set) in [("text", self.text), ("rodata", self.rodata), ("data", self.data)] {
            if !set.address.is_multiple_of(PAGE_SIZE) {
                return bad(format!("{name} is not page-aligned ({:#x})", set.address));
            }
            let reserved = u64::from(set.pages) * u64::from(PAGE_SIZE);
            if u64::from(set.size) > reserved {
                return bad(format!("{name} holds {:#x} bytes in {:#x} bytes of pages", set.size, reserved));
            }
            if u64::from(set.address) < previous_end {
                return bad(format!("{name} at {:#x} overlaps the segment before it", set.address));
            }
            previous_end = u64::from(set.address) + reserved;
        }
        if previous_end > 1 << 32 {
            return bad("segments extend past the 32-bit address space".into());
        }
        Ok(())
    }

    /// The address ranges the ARM11 kernel capabilities map (3dbrew
    /// "NCCH/Extended Header"): a pair of `0b11111111100x` descriptors per
    /// range, page indices in bits 0-19, the end exclusive; bit 20 of the
    /// start means read-only, of the end static (cacheable) rather than
    /// IO. A single `0b111111111110` descriptor maps one IO page (bit 20:
    /// read-only). An unpaired start is ignored.
    pub fn mappings(&self) -> Vec<Mapping> {
        let mut out = Vec::new();
        let mut caps = self.kernel_caps.iter().copied().peekable();
        while let Some(d) = caps.next() {
            let page = (d & 0xF_FFFF) << 12;
            let bit20 = d & (1 << 20) != 0;
            if d >> 21 == 0x7FC {
                if let Some(end) = caps.next_if(|e| e >> 21 == 0x7FC) {
                    let cacheable = end & (1 << 20) != 0;
                    out.push(Mapping { start: page, end: (end & 0xF_FFFF) << 12, read_only: bit20, cacheable });
                }
            } else if d >> 20 == 0xFFE {
                out.push(Mapping { start: page, end: page + PAGE_SIZE, read_only: bit20, cacheable: false });
            }
        }
        out
    }

    /// Bounds on the size of the decompressed code: the segments lie one
    /// after another in whole pages, except that data may stop at its last
    /// byte. (Both measured dumps hold all of data's pages.)
    pub fn code_size_range(&self) -> (u64, u64) {
        let page = u64::from(PAGE_SIZE);
        let before_data = (u64::from(self.text.pages) + u64::from(self.rodata.pages)) * page;
        (before_data + u64::from(self.data.size), before_data + u64::from(self.data.pages) * page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_descriptors_pair_into_ranges() {
        let mut e = parse(&synthetic()).unwrap();
        // two of Ultra Sun's: DSP memory read-write IO, VRAM read-only static; then an IO page
        e.kernel_caps = vec![0xFF81_FF50, 0xFF81_FF58, 0xFF91_F000, 0xFF91_F600, 0xFFE1_EC00, 0xFFFF_FFFF];
        assert_eq!(
            e.mappings(),
            [
                Mapping { start: 0x1FF5_0000, end: 0x1FF5_8000, read_only: false, cacheable: false },
                Mapping { start: 0x1F00_0000, end: 0x1F60_0000, read_only: true, cacheable: true },
                Mapping { start: 0x1EC0_0000, end: 0x1EC0_1000, read_only: false, cacheable: false },
            ]
        );
    }

    pub(crate) fn synthetic() -> Vec<u8> {
        let mut b = vec![0u8; SIZE];
        b[..4].copy_from_slice(b"test");
        let put = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 0x10, 0x0010_0000);
        put(&mut b, 0x14, 1);
        put(&mut b, 0x18, 0x800);
        put(&mut b, 0x20, 0x0010_1000);
        put(&mut b, 0x24, 1);
        put(&mut b, 0x28, 0x10);
        put(&mut b, 0x30, 0x0010_2000);
        put(&mut b, 0x34, 1);
        put(&mut b, 0x38, 0x20);
        b[0x250..0x258].copy_from_slice(b"fs:USER\0");
        b[0x258..0x260].copy_from_slice(b"hid:USER");
        b
    }

    #[test]
    fn parses_segments_and_services() {
        let e = parse(&synthetic()).unwrap();
        assert_eq!(e.process_name, "test");
        assert_eq!(e.text, CodeSet { address: 0x0010_0000, pages: 1, size: 0x800 });
        assert_eq!(e.services, ["fs:USER", "hid:USER"]);
        assert_eq!(e.code_size_range(), (0x2020, 0x3000));
        assert_eq!(e.kernel_version, None);
        let mut b = synthetic();
        b[0x370 + 8..0x370 + 12].copy_from_slice(&0xFC00_022Cu32.to_le_bytes());
        assert_eq!(parse(&b).unwrap().kernel_version, Some((2, 44)));
    }

    #[test]
    fn rejects_inconsistent_segments() {
        let mut b = synthetic();
        b[0x18..0x1C].copy_from_slice(&0x2000u32.to_le_bytes()); // text larger than its page
        assert!(parse(&b).is_err());
        let mut b = synthetic();
        b[0x20..0x24].copy_from_slice(&0x0010_0000u32.to_le_bytes()); // rodata over text
        assert!(parse(&b).is_err());
        assert!(parse(&b[..0x100]).is_err(), "short input");
    }
}

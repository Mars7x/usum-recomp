//! Bounds-checked access to an input file and to byte buffers.
//!
//! Every offset and length read from a dump is untrusted. All arithmetic on
//! them is checked, every read is checked against the file's real size
//! (reporting truncation), and no single read may exceed `MAX_READ`, so a
//! hostile length cannot make us allocate gigabytes.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::Error;

/// Largest single structure this crate reads into memory. The largest real
/// one is the decompressed executable (5.6 MiB for Ultra Sun); 3DS
/// applications cannot address more than 256 MiB of code anyway.
pub const MAX_READ: u64 = 64 * 1024 * 1024;

pub struct Source {
    file: File,
    len: u64,
}

impl Source {
    pub fn open(path: &Path) -> Result<Source, Error> {
        let file = File::open(path).map_err(Error::Io)?;
        let len = file.metadata().map_err(Error::Io)?.len();
        Ok(Source { file, len })
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    /// Reads `len` bytes at `offset`, which must lie inside the file.
    pub fn read_at(&mut self, what: &'static str, offset: u64, len: u64) -> Result<Vec<u8>, Error> {
        if len > MAX_READ {
            return Err(Error::malformed(what, format!("{len:#x} bytes is larger than any real structure")));
        }
        let end = offset.checked_add(len).ok_or_else(|| Error::malformed(what, "offset and size overflow"))?;
        if end > self.len {
            return Err(Error::Truncated { what, needed_end: end, file_size: self.len });
        }
        let mut bytes = vec![0; len as usize];
        self.file.seek(SeekFrom::Start(offset)).map_err(Error::Io)?;
        self.file.read_exact(&mut bytes).map_err(Error::Io)?;
        Ok(bytes)
    }
}

/// Little-endian fields of a buffer whose size has not been trusted.
#[derive(Clone, Copy)]
pub struct Le<'a> {
    pub what: &'static str,
    pub bytes: &'a [u8],
}

impl<'a> Le<'a> {
    pub fn new(what: &'static str, bytes: &'a [u8]) -> Le<'a> {
        Le { what, bytes }
    }

    pub fn slice(&self, offset: usize, len: usize) -> Result<&'a [u8], Error> {
        offset.checked_add(len).and_then(|end| self.bytes.get(offset..end)).ok_or_else(|| {
            Error::malformed(self.what, format!("{len} bytes at {offset:#x} lie outside its {} bytes", self.bytes.len()))
        })
    }

    fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N], Error> {
        Ok(self.slice(offset, N)?.try_into().expect("slice() returned exactly N bytes"))
    }

    pub fn u8(&self, offset: usize) -> Result<u8, Error> {
        Ok(self.array::<1>(offset)?[0])
    }

    pub fn u16(&self, offset: usize) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.array(offset)?))
    }

    pub fn u32(&self, offset: usize) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array(offset)?))
    }

    pub fn u64(&self, offset: usize) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array(offset)?))
    }

    pub fn hash(&self, offset: usize) -> Result<[u8; 32], Error> {
        self.array(offset)
    }

    /// A fixed-size, NUL-padded ASCII field. Non-ASCII bytes are malformed.
    pub fn ascii(&self, offset: usize, len: usize) -> Result<String, Error> {
        let raw = self.slice(offset, len)?;
        let raw = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
        if !raw.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
            return Err(Error::malformed(self.what, format!("the text field at {offset:#x} is not ASCII")));
        }
        Ok(String::from_utf8(raw.to_vec()).expect("checked ASCII"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fields_and_rejects_out_of_range() {
        let le = Le::new("test", &[1, 0, 0, 0, b'A', b'B', 0, 0]);
        assert_eq!(le.u32(0).unwrap(), 1);
        assert_eq!(le.ascii(4, 4).unwrap(), "AB");
        assert!(le.u32(5).is_err());
        assert!(le.slice(usize::MAX, 2).is_err());
        assert!(Le::new("t", &[0xFF, 0]).ascii(0, 2).is_err());
    }

    #[test]
    fn source_reports_truncation_and_huge_reads() {
        let path = crate::testutil::write_temp("source.bin", &[0u8; 100]);
        let mut source = Source::open(&path).unwrap();
        assert!(source.read_at("x", 90, 10).is_ok());
        assert!(matches!(source.read_at("x", 90, 11), Err(Error::Truncated { needed_end: 101, .. })));
        assert!(matches!(source.read_at("x", u64::MAX, 2), Err(Error::Malformed { .. })));
        assert!(matches!(source.read_at("x", 0, MAX_READ + 1), Err(Error::Malformed { .. })));
    }
}

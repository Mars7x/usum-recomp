//! `fs:USER`: archives and files (3dbrew "Filesystem services"). Commands
//! are added when the game first sends them.
//!
//! The game's RomFS is read from the user's dump in place, never copied
//! out of it: archive 3 (SelfNCCH) with an all-zero file path gives the
//! process the RomFS **level 3 image** (3dbrew: "the caller must parse the
//! filesystem inside the exposed image"), which is a range of the dump.

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

use super::Message;

/// The RomFS level 3 image, read from the dump on demand.
#[derive(Debug)]
pub struct Image {
    file: File,
    offset: u64,
    size: u64,
}

impl Image {
    /// `range` is where the image lies in the dump (from
    /// `usum_import::Inspection::romfs_image`).
    pub fn open(dump: &Path, range: usum_import::romfs::Image) -> io::Result<Image> {
        Ok(Image { file: File::open(dump)?, offset: range.offset, size: range.size })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Reads up to `len` bytes at `at`, fewer at the end of the image.
    pub fn read(&self, at: u64, len: u32) -> io::Result<Vec<u8>> {
        let len = u64::from(len).min(self.size.saturating_sub(at));
        let mut bytes = vec![0; len as usize];
        self.file.read_exact_at(&mut bytes, self.offset + at)?;
        Ok(bytes)
    }
}

pub enum Request {
    /// InitializeWithSdkVersion (0x0861): the SDK version, then a
    /// process-ID descriptor (0x20) the kernel fills in. Result only.
    InitializeWithSdkVersion {
        sdk_version: u32,
    },
    /// OpenFileDirectly (0x0803, 3dbrew "FS:OpenFileDirectly"): paths are
    /// (type, size, address) in guest memory.
    OpenFileDirectly {
        archive: u32,
        archive_path: Path3,
        file_path: Path3,
        flags: u32,
    },
    /// GetFormatInfo (0x0845, 3dbrew "FS:GetFormatInfo"): result, total
    /// size, directories, files, u8 duplicate data.
    GetFormatInfo {
        archive: u32,
        path: Path3,
    },
    /// FormatSaveData (0x084C, 3dbrew "FS:FormatSaveData"): archive, path,
    /// block size, directories, files, directory and file buckets,
    /// duplicate data.
    FormatSaveData {
        archive: u32,
        path: Path3,
        directories: u32,
        files: u32,
        duplicate: bool,
    },
    /// CreateFile (0x0808, 3dbrew "FS:CreateFile"): transaction, u64
    /// archive handle, path, attributes, u64 size.
    CreateFile {
        archive: u64,
        path: Path3,
        size: u64,
    },
    /// OpenFile (0x0802, 3dbrew "FS:OpenFile"): transaction, u64 archive
    /// handle, path, open flags, attributes.
    OpenFile {
        archive: u64,
        path: Path3,
        flags: u32,
    },
    /// CreateDirectory (0x0809): transaction, u64 archive handle, path,
    /// attributes.
    CreateDirectory {
        archive: u64,
        path: Path3,
    },
    /// OpenDirectory (0x080B): u64 archive handle, path.
    OpenDirectory {
        archive: u64,
        path: Path3,
    },
    /// CloseArchive (0x080E): u64 archive handle.
    CloseArchive(u64),
    /// DeleteFile (0x0804): transaction, u64 archive handle, path.
    DeleteFile {
        archive: u64,
        path: Path3,
    },
    /// OpenArchive (0x080C): archive ID and path; result, u64 archive handle.
    OpenArchive {
        archive: u32,
        path: Path3,
    },
    Other(u32),
}

/// An FS path as a request carries it: type, size in bytes, address.
#[derive(Debug, Clone, Copy)]
pub struct Path3 {
    pub kind: u32,
    pub size: u32,
    pub address: u32,
}

/// The translate descriptor asking the kernel for the caller's process ID
/// (3dbrew "IPC").
pub const PROCESS_ID: u32 = 0x20;

/// Archive IDs and path types (3dbrew "Filesystem services").
pub const ARCHIVE_SELF_NCCH: u32 = 3;
pub const ARCHIVE_SAVE_DATA: u32 = 4;
pub const ARCHIVE_EXT_SAVE_DATA: u32 = 6;

/// "Object doesn't exist / failed to open object" (3dbrew "Filesystem
/// services PXI" error table), description 120. Ultra Sun's code groups
/// FS descriptions 100-179 as one family, which this falls in (R-029).
pub const RESULT_NOT_FOUND: u32 = 0xC880_4478;

/// An archive as the game names it, mapped to a host directory under the
/// save root: `savedata/` for its save data, `extdata/<media>-<id>/` for
/// extra data. Never anywhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Archive {
    SaveData,
    ExtSaveData { media: u32, id: u64 },
}

impl Archive {
    /// From an archive ID and its path bytes, if this runtime knows it.
    pub fn named(archive: u32, kind: u32, path: &[u8]) -> Option<Archive> {
        match (archive, kind, path.len()) {
            (ARCHIVE_SAVE_DATA, PATH_EMPTY, _) => Some(Archive::SaveData),
            (ARCHIVE_EXT_SAVE_DATA, PATH_BINARY, 12) => {
                let word = |k: usize| u32::from_le_bytes(path[k..k + 4].try_into().expect("4"));
                Some(Archive::ExtSaveData { media: word(0), id: u64::from(word(8)) << 32 | u64::from(word(4)) })
            }
            _ => None,
        }
    }

    pub fn dir(&self, root: &Path) -> std::path::PathBuf {
        match self {
            Archive::SaveData => root.join("savedata"),
            Archive::ExtSaveData { media, id } => root.join("extdata").join(format!("{media}-{id:016x}")),
        }
    }
}

/// The result for an archive that has not been formatted: FS module (17),
/// description 340. HYPOTHESIS (R-029): 3dbrew lists no such code. Ultra
/// Sun's save code (0x00455C58) treats FS descriptions 340-359 as a state
/// of its own rather than an error, which fits "not formatted yet";
/// 0xC8A04554 is the commonly cited full code with that description.
pub const RESULT_NOT_FORMATTED: u32 = 0xC8A0_4554;

/// "Object does already exist / failed to create object" (3dbrew
/// "Filesystem services PXI").
pub const RESULT_ALREADY_EXISTS: u32 = 0xC820_44BE;

/// What FormatSaveData set up (3dbrew "FS:GetFormatInfo" fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub size: u32,
    pub directories: u32,
    pub files: u32,
    pub duplicate: bool,
}

impl Format {
    /// Kept beside the archive's directory (`savedata.format`), never in
    /// it, so no name the game uses can collide with it.
    fn file(dir: &Path) -> std::path::PathBuf {
        dir.with_extension("format")
    }

    /// The format recorded in a save directory, if it has been formatted.
    pub fn load(dir: &Path) -> Option<Format> {
        let text = std::fs::read_to_string(Format::file(dir)).ok()?;
        let v: Vec<u32> = text.split_whitespace().map(|w| w.parse().ok()).collect::<Option<_>>()?;
        match v[..] {
            [size, directories, files, duplicate] => Some(Format { size, directories, files, duplicate: duplicate != 0 }),
            _ => None,
        }
    }

    pub fn store(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = format!("{} {} {} {}\n", self.size, self.directories, self.files, u32::from(self.duplicate));
        std::fs::write(Format::file(dir), text)
    }
}
pub const PATH_EMPTY: u32 = 1;
pub const PATH_BINARY: u32 = 2;
pub const PATH_ASCII: u32 = 3;
pub const PATH_UTF16: u32 = 4;
pub const OPEN_READ: u32 = 1;
pub const OPEN_WRITE: u32 = 2;
pub const OPEN_CREATE: u32 = 4;

/// A guest path inside an archive, as host path components: ASCII or
/// UTF-16 (3dbrew "Filesystem services": sizes include the terminator),
/// absolute, '/'-separated. Refused: anything that could leave the
/// archive's directory or is not plain (".", "..", NUL, '\\', empty
/// components other than a trailing one). The game's paths never need
/// those; a refusal stops the run.
pub fn archive_path(kind: u32, bytes: &[u8]) -> Result<Vec<String>, String> {
    let text = match kind {
        PATH_ASCII => String::from_utf8(bytes.to_vec()).map_err(|_| "path is not ASCII".to_owned())?,
        PATH_UTF16 if bytes.len().is_multiple_of(2) => {
            let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            String::from_utf16(&units).map_err(|_| "path is not UTF-16".to_owned())?
        }
        _ => return Err(format!("path type {kind}")),
    };
    let text = text.strip_suffix('\0').unwrap_or(&text);
    let rest = text.strip_prefix('/').ok_or_else(|| format!("relative path {text:?}"))?;
    let parts: Vec<String> = rest.split('/').map(str::to_owned).collect();
    let (last, body) = parts.split_last().expect("split gives one");
    let plain = |p: &String| !p.is_empty() && p != "." && p != ".." && !p.contains(['\0', '\\']);
    if !body.iter().all(plain) || !(last.is_empty() || plain(last)) {
        return Err(format!("path {text:?}"));
    }
    Ok(parts.into_iter().filter(|p| !p.is_empty()).collect())
}

/// `parts` under `dir`.
pub fn host_path(dir: &Path, parts: &[String]) -> std::path::PathBuf {
    parts.iter().fold(dir.to_path_buf(), |p, part| p.join(part))
}

pub fn parse(message: &Message) -> Request {
    let w = &message.words;
    match w[0] {
        0x0861_0042 if w[2] == PROCESS_ID => Request::InitializeWithSdkVersion { sdk_version: w[1] },
        0x0803_0204 => Request::OpenFileDirectly {
            archive: w[2],
            archive_path: Path3 { kind: w[3], size: w[4], address: w[10] },
            file_path: Path3 { kind: w[5], size: w[6], address: w[12] },
            flags: w[7],
        },
        0x080C_00C2 if w[4] == (w[3] << 14) | 2 => {
            Request::OpenArchive { archive: w[1], path: Path3 { kind: w[2], size: w[3], address: w[5] } }
        }
        0x0804_0142 if w[6] == (w[5] << 14) | 2 => Request::DeleteFile {
            archive: u64::from(w[3]) << 32 | u64::from(w[2]),
            path: Path3 { kind: w[4], size: w[5], address: w[7] },
        },
        0x0808_0202 if w[9] == (w[5] << 14) | 2 => Request::CreateFile {
            archive: u64::from(w[3]) << 32 | u64::from(w[2]),
            path: Path3 { kind: w[4], size: w[5], address: w[10] },
            size: u64::from(w[8]) << 32 | u64::from(w[7]),
        },
        0x0802_01C2 if w[8] == (w[5] << 14) | 2 => Request::OpenFile {
            archive: u64::from(w[3]) << 32 | u64::from(w[2]),
            path: Path3 { kind: w[4], size: w[5], address: w[9] },
            flags: w[6],
        },
        0x0809_0182 if w[7] == (w[5] << 14) | 2 => Request::CreateDirectory {
            archive: u64::from(w[3]) << 32 | u64::from(w[2]),
            path: Path3 { kind: w[4], size: w[5], address: w[8] },
        },
        0x080B_0102 if w[5] == (w[4] << 14) | 2 => Request::OpenDirectory {
            archive: u64::from(w[2]) << 32 | u64::from(w[1]),
            path: Path3 { kind: w[3], size: w[4], address: w[6] },
        },
        0x080E_0080 => Request::CloseArchive(u64::from(w[2]) << 32 | u64::from(w[1])),
        0x084C_0242 if w[10] == (w[3] << 14) | 2 => Request::FormatSaveData {
            archive: w[1],
            path: Path3 { kind: w[2], size: w[3], address: w[11] },
            directories: w[5],
            files: w[6],
            duplicate: w[9] & 0xFF != 0,
        },
        0x0845_00C2 if w[4] == (w[3] << 14) | 2 => {
            Request::GetFormatInfo { archive: w[1], path: Path3 { kind: w[2], size: w[3], address: w[5] } }
        }
        other => Request::Other(other),
    }
}

/// OpenFile's reply: the same shape as OpenFileDirectly's.
pub fn open_file_reply_for(command: u16, handle: u32) -> Vec<u32> {
    vec![super::header(command, 1, 2), 0, super::srv::MOVE_HANDLE, handle]
}

/// 3dbrew "FS:OpenFileDirectly": result, an unused word (the move-handle
/// descriptor 0x10 in libctru's reading), the file session.
pub fn open_file_reply(handle: u32) -> Vec<u32> {
    vec![super::header(0x803, 1, 2), 0, super::srv::MOVE_HANDLE, handle]
}

/// A request on an open file's session (3dbrew "Filesystem services",
/// file commands).
pub enum FileRequest {
    /// Read (0x0802, "FSFile:Read"): offset, size, a mapped output buffer.
    Read {
        offset: u64,
        size: u32,
        buffer: u32,
    },
    /// Write (0x0803, "FSFile:Write"): offset, size, flush flags, a mapped
    /// input buffer.
    Write {
        offset: u64,
        size: u32,
        buffer: u32,
    },
    /// GetSize (0x0804): the size as a u64.
    GetSize,
    /// SetSize (0x0805): a u64.
    SetSize(u64),
    /// Close (0x0808).
    Close,
    /// Flush (0x0809).
    Flush,
    /// SetPriority (0x080A): the session's I/O priority. Result only.
    SetPriority(u32),
    Other(u32),
}

pub fn parse_file(message: &Message) -> FileRequest {
    let w = &message.words;
    match w[0] {
        0x0802_00C2 if w[4] == (w[3] << 4) | 0xC => {
            FileRequest::Read { offset: u64::from(w[2]) << 32 | u64::from(w[1]), size: w[3], buffer: w[5] }
        }
        0x0803_0102 if w[5] == (w[3] << 4) | 0xA => {
            FileRequest::Write { offset: u64::from(w[2]) << 32 | u64::from(w[1]), size: w[3], buffer: w[6] }
        }
        0x0804_0000 => FileRequest::GetSize,
        0x0805_0080 => FileRequest::SetSize(u64::from(w[2]) << 32 | u64::from(w[1])),
        0x0808_0000 => FileRequest::Close,
        0x0809_0000 => FileRequest::Flush,
        0x080A_0040 => FileRequest::SetPriority(w[1]),
        other => FileRequest::Other(other),
    }
}

/// "FSFile:Read" response: result, bytes read.
pub fn read_reply(read: u32) -> Vec<u32> {
    vec![super::header(0x802, 2, 0), 0, read]
}

/// GetSize response: result, size low, size high (libctru FSFILE_GetSize).
pub fn size_reply(size: u64) -> Vec<u32> {
    vec![super::header(0x804, 3, 0), 0, size as u32, (size >> 32) as u32]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_paths_stay_inside() {
        let utf16 = |s: &str| s.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>();
        assert_eq!(archive_path(PATH_UTF16, &utf16("/main")).unwrap(), ["main"]);
        assert_eq!(archive_path(PATH_ASCII, b"/a/b/\0").unwrap(), ["a", "b"]);
        assert!(archive_path(PATH_ASCII, b"/").unwrap().is_empty());
        for bad in ["main", "/../x", "/a/./b", "/a//b", "/a\\b"] {
            assert!(archive_path(PATH_ASCII, bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn image_reads_are_relative_and_clamped() {
        let path = std::env::temp_dir().join(format!("usum-fs-image-{}", std::process::id()));
        std::fs::write(&path, (0u8..32).collect::<Vec<u8>>()).unwrap();
        let image = Image::open(&path, usum_import::romfs::Image { offset: 8, size: 16 }).unwrap();
        assert_eq!(image.read(0, 4).unwrap(), [8, 9, 10, 11]);
        assert_eq!(image.read(14, 8).unwrap(), [22, 23], "stops at the end of the image");
        assert!(image.read(20, 4).unwrap().is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn file_read_requests_need_a_matching_buffer_descriptor() {
        let mut m = Message { words: [0; super::super::COMMAND_WORDS], statics: [0; super::super::STATIC_WORDS] };
        m.words[..6].copy_from_slice(&[0x0802_00C2, 0x10, 1, 0x200, (0x200 << 4) | 0xC, 0x0800_0000]);
        assert!(matches!(parse_file(&m), FileRequest::Read { offset: 0x1_0000_0010, size: 0x200, buffer: 0x0800_0000 }));
        m.words[4] = 0xA;
        assert!(matches!(parse_file(&m), FileRequest::Other(_)));
    }
}

/// One directory entry as FSDir:Read returns it (3dbrew "Filesystem
/// services", DirectoryEntry: 0x228 bytes).
pub const DIRECTORY_ENTRY_SIZE: usize = 0x228;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
    pub size: u64,
}

/// The entries of host directory `dir`, by name.
pub fn list(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let meta = e.metadata()?;
        out.push(Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            directory: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

impl Entry {
    /// The 0x228-byte form: UTF-16 name, 8.3 short name and extension (a
    /// truncated upper-case form: a choice, 3dbrew gives no rule), 1,
    /// reserved, attributes (directory, hidden, archive, read-only),
    /// size.
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0u8; DIRECTORY_ENTRY_SIZE];
        for (k, u) in self.name.encode_utf16().take(0x105).enumerate() {
            b[2 * k..2 * k + 2].copy_from_slice(&u.to_le_bytes());
        }
        let (stem, ext) = self.name.rsplit_once('.').filter(|_| !self.directory).unwrap_or((&self.name, ""));
        let short = |s: &str, n: usize| {
            s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase() as u8).take(n).collect::<Vec<u8>>()
        };
        let (s8, s3) = (short(stem, 8), short(ext, 3));
        b[0x20C..0x20C + s8.len()].copy_from_slice(&s8);
        b[0x216..0x216 + s3.len()].copy_from_slice(&s3);
        b[0x21A] = 1;
        b[0x21C] = u8::from(self.directory);
        b[0x21E] = u8::from(!self.directory);
        b[0x220..0x228].copy_from_slice(&self.size.to_le_bytes());
        b
    }
}

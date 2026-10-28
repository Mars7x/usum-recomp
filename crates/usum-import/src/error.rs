use std::fmt;

/// Everything that can be wrong with an input. Messages are written for
/// the person who supplied the dump, and say what to do where possible.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// Not any container this project reads.
    NotADump(&'static str),
    /// A format this project recognizes but does not read (yet).
    UnsupportedFormat(&'static str),
    /// The file ends before a structure it declares.
    Truncated {
        what: &'static str,
        needed_end: u64,
        file_size: u64,
    },
    /// A field holds a value that cannot be right.
    Malformed {
        what: &'static str,
        detail: String,
    },
    /// The content is encrypted; this project never handles keys.
    Encrypted,
    /// A stored SHA-256 does not match the data it covers.
    HashMismatch {
        what: String,
    },
    NotExecutable,
    IsUpdate(u64),
    IsDlc(u64),
}

impl Error {
    pub(crate) fn malformed(what: &'static str, detail: impl Into<String>) -> Error {
        Error::Malformed { what, detail: detail.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "could not read the file: {e}"),
            Error::NotADump(why) => write!(f, "{why}"),
            Error::UnsupportedFormat(why) => write!(f, "{why}"),
            Error::Truncated { what, needed_end, file_size } => write!(
                f,
                "the file is incomplete: {what} ends at byte {needed_end:#x}, but the file is only {file_size:#x} bytes. \
                 Dump the cartridge again"
            ),
            Error::Malformed { what, detail } => write!(f, "malformed {what}: {detail}"),
            Error::Encrypted => write!(
                f,
                "the dump is encrypted. Decrypt it on your own console first (for example with GodMode9's \
                 \"Decrypt file\"); this project never handles keys"
            ),
            Error::HashMismatch { what } => {
                write!(f, "{what} does not match its stored SHA-256: the dump is damaged or was modified. Dump it again")
            }
            Error::NotExecutable => write!(f, "the game partition is not an executable (CXI)"),
            Error::IsUpdate(id) => {
                write!(f, "{id:016X} is an update title; select the base game's dump (updates are not supported yet)")
            }
            Error::IsDlc(id) => write!(f, "{id:016X} is downloadable content, not a game"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

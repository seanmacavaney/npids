//! npids: time- and space-efficient bidirectional lookups between dense indices and IDs.
//!
//! The on-disk format is specified in `FORMAT.md`.

pub mod bits;
pub mod chunk;
pub mod format;
pub mod infer;
pub mod phf;
pub mod reader;
pub mod template;
pub mod writer;

#[cfg(feature = "python")]
mod python;

pub use reader::Reader;
pub use writer::{Options, Writer, compact};

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The file is damaged or not a valid npids file.
    Corrupt(String),
    /// The file uses a version or feature this library does not support.
    Unsupported(String),
    /// Invalid arguments.
    Invalid(String),
    /// An index beyond the end of the file.
    OutOfRange(u64),
    /// An ID was not found, but some indices are not covered by an inverse unit.
    NoInverse,
}

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Corrupt(m) => write!(f, "corrupt npids file: {m}"),
            Error::Unsupported(m) => write!(f, "{m}"),
            Error::Invalid(m) => write!(f, "{m}"),
            Error::OutOfRange(i) => write!(f, "index {i} is out of range"),
            Error::NoInverse => write!(f, "inverse lookups are not available for every ID in this file (it was built without them)"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

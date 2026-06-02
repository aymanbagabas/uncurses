use std::fmt;
use std::io;

/// Errors produced by image source decoding. Frame-level operations
/// (`paint`, `render`) return `io::Result<()>` directly — they only
/// fail for terminal write errors.
#[derive(Debug)]
pub enum Error {
    /// Decoding the image bytes failed.
    Decode(image::ImageError),
    /// I/O failure reading an image source from disk.
    Io(io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "image decode failed: {e}"),
            Self::Io(e) => write!(f, "image I/O failed: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(e) => Some(e),
            Self::Io(e) => Some(e),
        }
    }
}

impl From<image::ImageError> for Error {
    fn from(value: image::ImageError) -> Self {
        Self::Decode(value)
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

use std::{ffi::OsStr, fs, io, path::Path};
use url::Url;

/// Convert an existing local path to a file URI.
///
/// Resolve relative paths and parent directories while preserving the final
/// symlink, unless a trailing separator or `/.` requests its target directory.
/// Inspect the supplied path with [`fs::symlink_metadata`] and append `/` for
/// directories. Unix filename bytes are percent-encoded without replacement.
/// File contents are not read. Path resolution, metadata, and conversion
/// errors are returned to the caller.
pub fn path_to_uri(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref();
    let metadata = fs::symlink_metadata(path)?;
    let path = if metadata.is_dir() {
        fs::canonicalize(path)?
    } else {
        let path = std::path::absolute(path)?;
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => fs::canonicalize(parent)?.join(name),
            _ => fs::canonicalize(path)?,
        }
    };
    let uri = if metadata.is_dir() {
        Url::from_directory_path(path)
    } else {
        Url::from_file_path(path)
    };
    uri.map(String::from).map_err(|()| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "path cannot be represented as a file URI",
        )
    })
}

pub(crate) fn os_bytes(value: &OsStr) -> io::Result<&[u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(value.as_bytes())
    }
    #[cfg(not(unix))]
    {
        value.to_str().map(str::as_bytes).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "filename is not valid UTF-8")
        })
    }
}

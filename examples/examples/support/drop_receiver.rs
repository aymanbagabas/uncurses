use std::{
    collections::{HashMap, VecDeque},
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

use percent_encoding::percent_decode_str;
use tempfile::TempDir;
use uncurses_dragdrop::{Directory, DragDropOptions, Entry, EntryKind};
use url::Url;

const PREVIEW_LIMIT: usize = 256;
const LINK_LIMIT: usize = 64 * 1024;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn native_name(bytes: Vec<u8>) -> io::Result<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes)
            .map(OsString::from)
            .map_err(|_| invalid("the native filename is not valid UTF-8"))
    }
}

fn component(name: &OsStr) -> io::Result<()> {
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(Component::Normal(part)) if part == name)
        || parts.next().is_some()
        || name.as_encoded_bytes().contains(&0)
    {
        return Err(invalid("a directory child must be one filename"));
    }
    #[cfg(windows)]
    {
        let name = name
            .to_str()
            .ok_or_else(|| invalid("invalid Windows filename"))?;
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if name.contains([':', '<', '>', '"', '|', '?', '*'])
            || name.ends_with(['.', ' '])
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(
                    &stem[3..],
                    "1" | "2"
                        | "3"
                        | "4"
                        | "5"
                        | "6"
                        | "7"
                        | "8"
                        | "9"
                        | "\u{b9}"
                        | "\u{b2}"
                        | "\u{b3}"
                )
        {
            return Err(invalid("unsupported Windows filename"));
        }
    }
    Ok(())
}

fn root_name(uri: &[u8]) -> io::Result<OsString> {
    let uri = std::str::from_utf8(uri).map_err(|_| invalid("invalid file URI"))?;
    let uri = Url::parse(uri).map_err(|_| invalid("invalid file URI"))?;
    if uri.scheme() != "file" {
        return Err(invalid("only file URIs can supply file entries"));
    }
    let name = uri
        .path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        .ok_or_else(|| invalid("the file URI must name a file or directory"))?;
    let name = native_name(percent_decode_str(name).collect())?;
    component(&name)?;
    Ok(name)
}

#[derive(Default)]
pub struct Receiver {
    pub active: bool,
    storage: Option<TempDir>,
    file: Option<File>,
    link: Option<Vec<u8>>,
    current: Option<PathBuf>,
    current_parent: Option<Directory>,
    listing: Option<Directory>,
    pending: VecDeque<(Entry, PathBuf, Option<Directory>)>,
    directories: HashMap<Directory, usize>,
    pub closed: Vec<Directory>,
    pub bytes: usize,
    pub listed: usize,
    pub preview: String,
    preview_full: bool,
    pub checksum: u64,
}

impl Receiver {
    pub fn begin(&mut self, mime: &str, destination: Option<&Path>) -> io::Result<()> {
        self.abort()?;
        self.active = true;
        self.checksum = 0xcbf29ce484222325;
        if let Some(destination) = destination {
            fs::create_dir_all(destination)?;
            self.storage = Some(
                tempfile::Builder::new()
                    .prefix("drop-")
                    .tempdir_in(destination)?,
            );
            let filename = match mime {
                "text/uri-list" => return Ok(()),
                "text/plain" | "text/plain;charset=utf-8" | "UTF8_STRING" => "text.txt",
                "text/html" => "content.html",
                "image/png" => "image.png",
                "image/jpeg" => "image.jpg",
                _ => return Err(invalid("unsupported MIME type")),
            };
            self.file = Some(self.create_file(Path::new(filename))?);
        }
        Ok(())
    }

    fn create_file(&self, relative: &Path) -> io::Result<File> {
        File::options()
            .write(true)
            .create_new(true)
            .open(self.storage.as_ref().unwrap().path().join(relative))
    }

    pub fn listed(
        &mut self,
        entry: Entry,
        name: Vec<u8>,
        options: &DragDropOptions,
    ) -> io::Result<bool> {
        let relative = match &entry {
            Entry::File(file) if file.root_index().is_some() => match root_name(&name) {
                Ok(name) => PathBuf::from(name),
                Err(_) => return Ok(false),
            },
            Entry::File(_) => {
                let name = native_name(name)?;
                component(&name)?;
                self.current
                    .as_ref()
                    .ok_or_else(|| invalid("child listing outside a directory"))?
                    .join(name)
            }
            _ => return Err(invalid("expected a file entry")),
        };
        if options
            .max_path_bytes
            .is_some_and(|limit| relative.as_os_str().as_encoded_bytes().len() > limit)
        {
            return Err(io::Error::new(
                io::ErrorKind::QuotaExceeded,
                "destination path limit exceeded",
            ));
        }
        if let Some(directory) = self.listing {
            let children = self.directories.get_mut(&directory).unwrap();
            self.pending
                .insert(*children, (entry, relative, self.listing));
            *children += 1;
        } else {
            self.pending.push_back((entry, relative, self.listing));
        }
        self.listed += 1;
        Ok(true)
    }

    pub fn next(&mut self) -> Option<Entry> {
        let (entry, path, parent) = self.pending.pop_front()?;
        self.current = Some(path);
        self.current_parent = parent;
        Some(entry)
    }

    pub fn kind(&mut self, kind: EntryKind) -> io::Result<()> {
        let relative = self
            .current
            .as_ref()
            .ok_or_else(|| invalid("entry kind without a pending file"))?;
        match kind {
            EntryKind::File => {
                if self.storage.is_some() {
                    self.file = Some(self.create_file(relative)?);
                }
            }
            EntryKind::Directory(directory) => {
                if let Some(storage) = &self.storage {
                    fs::create_dir(storage.path().join(relative))?;
                }
                self.directories.insert(directory, 0);
                self.listing = Some(directory);
            }
            EntryKind::Symlink => self.link = Some(Vec::new()),
        }
        Ok(())
    }

    pub fn data(&mut self, bytes: &[u8]) -> io::Result<()> {
        if let Some(file) = &mut self.file {
            file.write_all(bytes)?;
        }
        if let Some(link) = &mut self.link {
            if bytes.len() > LINK_LIMIT - link.len() {
                return Err(invalid("symlink target exceeds 64 KiB"));
            }
            link.extend_from_slice(bytes);
        }
        self.bytes += bytes.len();
        for byte in bytes {
            self.checksum = (self.checksum ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
            let escaped = byte.escape_ascii();
            if !self.preview_full {
                if self.preview.len() + escaped.len() <= PREVIEW_LIMIT {
                    self.preview.extend(escaped.map(char::from));
                } else {
                    self.preview_full = true;
                }
            }
        }
        Ok(())
    }

    pub fn end_body(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        if let Some(bytes) = self.link.take()
            && let Some(storage) = &self.storage
        {
            let relative = self.current.as_ref().unwrap();
            let target = PathBuf::from(native_name(bytes)?);
            let mut depth = relative.parent().unwrap().components().count();
            let mut named = false;
            for part in target.components() {
                match part {
                    Component::Normal(_) => named = true,
                    Component::CurDir => {}
                    Component::ParentDir if !named && depth > 0 => depth -= 1,
                    _ => return Err(invalid("symlink target can escape the drop directory")),
                }
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, storage.path().join(relative))?;
            #[cfg(not(unix))]
            {
                let _ = storage;
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "this example saves symlinks on Unix only",
                ));
            }
        }
        if let Some(parent) = self.current_parent.take() {
            *self.directories.get_mut(&parent).unwrap() -= 1;
            self.close_if_consumed(parent);
        }
        if let Some(directory) = self.listing.take() {
            self.close_if_consumed(directory);
        }
        self.current = None;
        Ok(())
    }

    fn close_if_consumed(&mut self, directory: Directory) {
        if self.directories[&directory] == 0 {
            self.directories.remove(&directory);
            self.closed.push(directory);
        }
    }

    pub fn finish(&mut self) -> Option<PathBuf> {
        self.active = false;
        self.storage.take().map(TempDir::keep)
    }

    pub fn abort(&mut self) -> io::Result<()> {
        self.file = None;
        let cleanup = self.storage.take().map(TempDir::close).transpose();
        *self = Self::default();
        cleanup.map(|_| ())
    }
}

#[cfg(test)]
#[path = "drop_receiver_tests.rs"]
mod tests;

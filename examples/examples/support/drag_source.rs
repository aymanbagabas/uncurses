use std::{
    collections::HashMap,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

#[derive(Default)]
pub struct DirectoryPool {
    handles: Vec<Weak<Dir>>,
    roots: HashMap<PathBuf, Weak<Dir>>,
    limit: Option<usize>,
}

impl DirectoryPool {
    pub fn new(limit: Option<usize>) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    fn open(&mut self, open: impl FnOnce() -> io::Result<Dir>) -> io::Result<Arc<Dir>> {
        self.handles.retain(|handle| handle.strong_count() > 0);
        if self.limit.is_some_and(|limit| self.handles.len() >= limit) {
            return Err(io::Error::new(
                io::ErrorKind::QuotaExceeded,
                "outgoing directory handle limit exceeded",
            ));
        }
        let directory = Arc::new(open()?);
        self.handles.push(Arc::downgrade(&directory));
        Ok(directory)
    }

    fn root(&mut self, path: &Path) -> io::Result<Arc<Dir>> {
        let path = std::fs::canonicalize(path)?;
        if let Some(directory) = self.roots.get(&path).and_then(Weak::upgrade) {
            return Ok(directory);
        }
        let directory = self.open(|| Dir::open_ambient_dir(&path, ambient_authority()))?;
        self.roots.insert(path, Arc::downgrade(&directory));
        Ok(directory)
    }
}

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};

#[derive(Clone)]
pub struct Source {
    parent: Arc<Dir>,
    pub name: OsString,
}

pub enum Data {
    File(File),
    Directory(Vec<Source>),
    Symlink(PathBuf),
}

impl Source {
    pub fn new(path: &Path, pool: &mut DirectoryPool) -> io::Result<Self> {
        let bytes = path.as_os_str().as_encoded_bytes();
        let trailing_separator = bytes.ends_with(b"/")
            || bytes.ends_with(b"/.")
            || cfg!(windows) && (bytes.ends_with(b"\\") || bytes.ends_with(b"\\."));
        let path = if trailing_separator {
            std::fs::canonicalize(path)?
        } else {
            std::path::absolute(path)?
        };
        let (parent, name) = if path.file_name().is_none() || trailing_separator {
            // A supplied directory path can explicitly follow a final link with `/`.
            (path.as_path(), OsString::from("."))
        } else {
            (
                path.parent().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "source has no parent")
                })?,
                path.file_name()
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "source has no filename")
                    })?
                    .to_owned(),
            )
        };
        Ok(Self {
            parent: pool.root(parent)?,
            name,
        })
    }

    pub fn read(&self, pool: &mut DirectoryPool) -> io::Result<Data> {
        let metadata = self.parent.symlink_metadata(&self.name)?;
        if metadata.is_symlink() {
            Ok(Data::Symlink(self.parent.read_link_contents(&self.name)?))
        } else if metadata.is_dir() {
            let parent = pool.open(|| self.parent.open_dir_nofollow(&self.name))?;
            let mut children = Vec::new();
            for entry in parent.entries()? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_file() || kind.is_dir() || kind.is_symlink() {
                    children.push(Self {
                        name: entry.file_name(),
                        parent: Arc::clone(&parent),
                    });
                }
            }
            children.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(Data::Directory(children))
        } else if metadata.is_file() {
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                // A raced FIFO must not block before the opened-file check.
                options.custom_flags(libc::O_NONBLOCK);
            }
            let file = self.parent.open_with(&self.name, &options)?;
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "source changed type",
                ));
            }
            Ok(Data::File(file))
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only regular files, directories, and symlinks are supported",
            ))
        }
    }
}

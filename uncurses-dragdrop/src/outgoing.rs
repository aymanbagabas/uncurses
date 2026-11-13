use crate::controller::{Drag, DragPhase, FailedBody};
use crate::path::os_bytes;
use crate::protocol::{BASE64, UriCounter, error_payload};
use crate::{DragDrop, Entry, Error, Operations, RemoteError, ThumbnailFormat, path_to_uri};
use base64::Engine;
use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    path::Path,
};

impl DragDrop {
    /// Prepare a URI offer and pre-send its complete URI list.
    ///
    /// Supply directory URLs with a trailing `/`. The controller transmits
    /// URLs as supplied and does not inspect or normalize filesystem paths.
    /// Return roots in supplied URI order. Associate each root with its source
    /// until the exchange ends, including repeated requests for the same root.
    pub fn offer_files(
        &mut self,
        out: &mut impl Write,
        uris: &[&str],
        operations: Operations,
    ) -> Result<Vec<Entry>, Error> {
        if uris.is_empty()
            || u32::try_from(uris.len()).is_err()
            || uris.iter().any(|uri| {
                uri.starts_with('#')
                    || uri.bytes().all(|b| matches!(b, b' ' | b'\t'))
                    || uri.bytes().any(|b| matches!(b, 0 | b'\r' | b'\n'))
            })
        {
            return Err(Error::InvalidInput(
                "expected nonempty URI entries without line separators",
            ));
        }
        self.offer(out, &["text/uri-list"], operations)?;
        self.send_from(out, Entry::UriList, &mut Separated::new(uris, b"\r\n"))?;
        Ok((1..=uris.len() as u32)
            .map(|index| Entry::file(self.drag.scope, 0, index))
            .collect())
    }

    /// Offer local paths and return their root entries in supplied order.
    ///
    /// Resolve and inspect every path with [`path_to_uri`] before protocol
    /// output. The application opens files and traverses directories when
    /// requested. Keep the returned roots associated with the source paths
    /// until the outgoing exchange ends.
    pub fn offer_paths(
        &mut self,
        out: &mut impl Write,
        paths: impl IntoIterator<Item = impl AsRef<Path>>,
        operations: Operations,
    ) -> Result<Vec<Entry>, Error> {
        let uris = paths
            .into_iter()
            .map(path_to_uri)
            .collect::<io::Result<Vec<_>>>()?;
        let uris: Vec<_> = uris.iter().map(String::as_str).collect();
        self.offer_files(out, &uris, operations)
    }

    /// Return the next eligible outgoing file target without consuming it.
    ///
    /// Roots follow terminal request order. Children follow breadth-first
    /// order, and a complete tree precedes the next root. Successful responses
    /// advance the queue. A directory listing makes child work available
    /// immediately, without another terminal event. Check pending work before
    /// blocking for input. Whole-body sends remain synchronous.
    ///
    /// Report a failed body or cancel the drag before requesting further work.
    pub fn next_send(&self) -> Result<Option<Entry>, Error> {
        self.ready_for_body()?;
        Ok(self.drag.children.front().cloned().or_else(|| {
            self.drag
                .file_requests
                .front()
                .map(|&index| Entry::file(self.drag.scope, 0, index))
        }))
    }

    /// Send a complete MIME body or regular file, including its end marker.
    ///
    /// During preparation, MIME targets supply initial data. During an active
    /// drag, MIME targets answer [`crate::Event::DataRequested`]. File targets answer
    /// [`crate::Event::EntryRequested`] or supply pending directory children.
    pub fn send(
        &mut self,
        out: &mut impl Write,
        entry: impl Into<Entry>,
        mut bytes: &[u8],
    ) -> Result<(), Error> {
        self.send_from(out, entry, &mut bytes)
    }

    /// Stream a complete body from a reader with bounded data buffers.
    ///
    /// This synchronous call reads until EOF. Slow reads or writes block the
    /// caller. Interrupted reads retry. Other source errors leave the body
    /// unfinished and block further data. Use [`Self::fail_data`] for a MIME
    /// request, or [`Self::fail_drag`] or [`Self::cancel_drag`] for other bodies.
    /// Output errors poison the controller because terminal state is uncertain.
    pub fn send_from(
        &mut self,
        out: &mut impl Write,
        entry: impl Into<Entry>,
        source: &mut impl Read,
    ) -> Result<(), Error> {
        self.ready_for_body()?;
        let entry = entry.into();
        if let Some(mime) = entry.mime() {
            let (index, initial) = self.data_target(mime)?;
            if initial && entry.is_uri_list() && self.drag.uri_count.is_some() {
                return Err(Error::InvalidState(
                    "the initial URI list is already complete",
                ));
            }
            let kind = if initial { "t=p" } else { "t=e" };
            let index_option = format!("{}={index}", if initial { "x" } else { "y" });
            let check = if initial && entry.is_uri_list() {
                BodyCheck::Uris(UriCounter::default())
            } else {
                BodyCheck::Any
            };
            let uri_count = self.write_body(
                out,
                &[kind, &index_option],
                source,
                check,
                FailedBody::Mime(index),
            )?;
            if initial && entry.is_uri_list() {
                self.drag.uri_count = uri_count;
            }
            if !initial {
                let pending = self
                    .drag
                    .requests
                    .iter()
                    .position(|requested| *requested == index)
                    .expect("data_target verified the pending request");
                self.drag.requests.remove(pending);
            }
        } else {
            self.send_file_body(out, &entry, 0, source)?;
            self.finish_file(&entry, None);
        }
        Ok(())
    }

    fn file_target(&self, entry: &Entry) -> Result<Vec<String>, Error> {
        if self.drag.phase != DragPhase::Active {
            return Err(Error::InvalidState("file data requires an active drag"));
        }
        let root = *self
            .drag
            .file_requests
            .front()
            .ok_or(Error::InvalidState("no URI entry was requested"))?;
        let mut metadata = vec!["t=k".to_owned(), format!("x={root}")];
        match entry {
            Entry::File(file)
                if file.scope == self.drag.scope
                    && file.parent == 0
                    && file.index == root
                    && self.drag.children.is_empty() => {}
            Entry::File(file)
                if file.scope == self.drag.scope
                    && self.drag.children.front() == Some(entry)
                    && self.drag.directories.get(&file.parent) == Some(&root) =>
            {
                metadata.push(format!("Y={}", file.parent));
                metadata.push(format!("y={}", file.index));
            }
            _ => {
                return Err(Error::InvalidInput(
                    "send file entries in request and breadth-first order",
                ));
            }
        }
        Ok(metadata)
    }

    fn send_file_body(
        &mut self,
        out: &mut impl Write,
        entry: &Entry,
        kind: i64,
        source: &mut impl Read,
    ) -> Result<(), Error> {
        self.ready_for_body()?;
        let mut metadata = self.file_target(entry)?;
        if kind != 0 {
            metadata.push(format!("X={kind}"));
        }
        self.write_body(
            out,
            &metadata.iter().map(String::as_str).collect::<Vec<_>>(),
            source,
            BodyCheck::Any,
            FailedBody::Global,
        )?;
        Ok(())
    }

    fn finish_file(&mut self, entry: &Entry, directory: Option<(i64, &[Entry])>) {
        if matches!(entry, Entry::File(file) if file.parent != 0) {
            self.drag.children.pop_front();
        }
        if let Some((handle, children)) = directory {
            let root = *self
                .drag
                .file_requests
                .front()
                .expect("file_target verified request");
            self.drag.directories.insert(handle, root);
            self.drag.children.extend(children.iter().cloned());
        }
        if self.drag.children.is_empty() {
            self.drag.file_requests.pop_front();
            self.drag.directories.clear();
        }
    }

    /// Send a native symbolic link target, such as the result of [`std::fs::read_link`].
    pub fn send_symlink(
        &mut self,
        out: &mut impl Write,
        entry: impl Into<Entry>,
        target: impl AsRef<Path>,
    ) -> Result<(), Error> {
        self.send_symlink_bytes(out, entry, os_bytes(target.as_ref().as_os_str())?)
    }

    /// Send uninterpreted symbolic link target bytes for a pending file entry.
    pub fn send_symlink_bytes(
        &mut self,
        out: &mut impl Write,
        entry: impl Into<Entry>,
        mut target: &[u8],
    ) -> Result<(), Error> {
        let entry = entry.into();
        self.send_file_body(out, &entry, 1, &mut target)?;
        self.finish_file(&entry, None);
        Ok(())
    }

    /// Send native child names and return their entries in supplied order.
    ///
    /// Accept names from [`std::fs::DirEntry::file_name`]. Unix names retain their
    /// original bytes. Other platforms require UTF-8 names and report a
    /// conversion error instead of replacing characters.
    /// Associate the returned entries with application sources, then use
    /// [`Self::next_send`] to select work. An empty directory completes immediately.
    pub fn send_directory(
        &mut self,
        out: &mut impl Write,
        entry: impl Into<Entry>,
        names: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) -> Result<Vec<Entry>, Error> {
        let names: Vec<_> = names.into_iter().collect();
        let names = names
            .iter()
            .map(|name| os_bytes(name.as_ref()))
            .collect::<io::Result<Vec<_>>>()?;
        self.send_directory_bytes(out, entry, &names)
    }

    /// Send binary child names and return their entries in supplied order.
    ///
    /// Names must be nonempty and contain no NUL. The application supplies
    /// every child response, in the order selected by [`Self::next_send`].
    pub fn send_directory_bytes(
        &mut self,
        out: &mut impl Write,
        entry: impl Into<Entry>,
        names: &[&[u8]],
    ) -> Result<Vec<Entry>, Error> {
        let count = u32::try_from(names.len())
            .map_err(|_| Error::InvalidInput("too many directory entries"))?;
        if names
            .iter()
            .any(|name| name.is_empty() || name.contains(&0))
        {
            return Err(Error::InvalidInput(
                "directory names must be nonempty and contain no NUL",
            ));
        }
        let handle = self.drag.next_handle.max(2);
        let next = handle
            .checked_add(1)
            .ok_or(Error::InvalidState("directory handles exhausted"))?;
        let entry = entry.into();
        self.send_file_body(
            out,
            &entry,
            i64::from(handle),
            &mut Separated::new(names, b"\0"),
        )?;
        self.drag.next_handle = next;
        let children: Vec<_> = (1..=count)
            .map(|index| Entry::file(self.drag.scope, i64::from(handle), index))
            .collect();
        self.finish_file(&entry, Some((i64::from(handle), &children)));
        Ok(children)
    }

    /// Abort the whole outgoing drag with a global protocol error.
    ///
    /// This also discards queued URI requests and pending directory children.
    /// Use after the startup request. Cancel preparation with [`Self::cancel_drag`].
    pub fn fail_drag(&mut self, out: &mut impl Write, error: RemoteError) -> Result<(), Error> {
        let payload = error_payload(&error)?;
        if !matches!(self.drag.phase, DragPhase::Starting | DragPhase::Active) {
            return Err(Error::InvalidState(
                "global errors require a started drag; cancel preparation",
            ));
        }
        self.write(out, &["t=E"], payload.as_bytes())?;
        self.drag = Drag::default();
        self.discard_continuation(&["o", "e", "E", "k"]);
        Ok(())
    }

    /// Add a thumbnail during preparation and return its zero-based index.
    ///
    /// RGB and RGBA bodies must exactly match their dimensions. Text must be
    /// UTF-8. PNG bytes pass through without image decoding.
    pub fn add_thumbnail(
        &mut self,
        out: &mut impl Write,
        format: ThumbnailFormat,
        mut bytes: &[u8],
    ) -> Result<u32, Error> {
        let (_, _, _, _, check) = format.wire()?;
        match check {
            BodyCheck::Image(Some(length)) if length != bytes.len() as u64 => {
                return Err(Error::InvalidInput(
                    "thumbnail byte count differs from dimensions",
                ));
            }
            BodyCheck::Text(_) if std::str::from_utf8(bytes).is_err() => {
                return Err(Error::InvalidInput("thumbnail text is not UTF-8"));
            }
            _ => {}
        }
        self.add_thumbnail_from(out, format, &mut bytes)
    }

    /// Stream a thumbnail during preparation with bounded buffers.
    ///
    /// Source errors or invalid content leave the body unfinished. If output
    /// started, the controller is poisoned: recover the terminal before reuse.
    /// Otherwise, cancel preparation. No successful end marker is emitted.
    pub fn add_thumbnail_from(
        &mut self,
        out: &mut impl Write,
        format: ThumbnailFormat,
        source: &mut impl Read,
    ) -> Result<u32, Error> {
        self.ready_for_body()?;
        if self.drag.phase != DragPhase::Preparing {
            return Err(Error::InvalidState("thumbnails require a prepared offer"));
        }
        let (y, width, height, opacity, check) = format.wire()?;
        let index = self.drag.thumbnails;
        let wire_index = -i64::from(index) - 1;
        if wire_index < i64::from(i32::MIN) {
            return Err(Error::InvalidState("thumbnail indices exhausted"));
        }
        let metadata = [
            "t=p".to_owned(),
            format!("x={wire_index}"),
            format!("y={y}"),
            format!("X={width}"),
            format!("Y={height}"),
            format!("o={opacity}"),
        ];
        self.write_body(
            out,
            &metadata.iter().map(String::as_str).collect::<Vec<_>>(),
            source,
            check,
            FailedBody::Global,
        )?;
        self.drag.thumbnails += 1;
        Ok(index)
    }

    /// Select an existing zero-based thumbnail during an active drag.
    /// Use `None` to remove the image.
    pub fn select_thumbnail(
        &mut self,
        out: &mut impl Write,
        index: Option<u32>,
    ) -> Result<(), Error> {
        if self.drag.phase != DragPhase::Active {
            return Err(Error::InvalidState(
                "thumbnail selection requires an active drag",
            ));
        }
        if index.is_some_and(|index| index >= self.drag.thumbnails) {
            return Err(Error::InvalidInput("thumbnail index is out of range"));
        }
        self.write(
            out,
            &[
                "t=P",
                &format!("x={}", index.unwrap_or(self.drag.thumbnails)),
            ],
            b"",
        )
    }
}

impl ThumbnailFormat {
    fn wire(self) -> Result<(u32, u32, u32, u16, BodyCheck), Error> {
        let (kind, width, height, opacity, channels) = match self {
            Self::Rgb { width, height } => (24, width, height, 0, 3),
            Self::Rgba { width, height } => (32, width, height, 0, 4),
            Self::Png { width, height } => (100, width, height, 0, 0),
            Self::Text {
                numerator,
                denominator,
                opacity,
            } => (0, numerator, denominator, opacity, 0),
        };
        if width == 0 || height == 0 || opacity > 1024 {
            return Err(Error::InvalidInput(
                "invalid thumbnail dimensions, ratio, or opacity",
            ));
        }
        let check = if kind == 0 {
            BodyCheck::Text(Vec::new())
        } else {
            // Validate the expanded RGBA size, including PNG dimensions.
            u64::from(width)
                .checked_mul(u64::from(height))
                .and_then(|n| n.checked_mul(4))
                .and_then(|n| usize::try_from(n).ok())
                .ok_or(Error::InvalidInput("thumbnail dimensions overflow"))?;
            if channels == 0 {
                BodyCheck::Image(None)
            } else {
                BodyCheck::Image(Some(u64::from(width) * u64::from(height) * channels))
            }
        };
        Ok((kind, width, height, opacity, check))
    }
}

struct Separated<'a, T> {
    parts: &'a [T],
    separator: &'a [u8],
    part: usize,
    offset: usize,
}

impl<'a, T> Separated<'a, T> {
    fn new(parts: &'a [T], separator: &'a [u8]) -> Self {
        Self {
            parts,
            separator,
            part: 0,
            offset: 0,
        }
    }
}

impl<T: AsRef<[u8]>> Read for Separated<'_, T> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut used = 0;
        while used < buffer.len() && self.part < self.parts.len() {
            let part = self.parts[self.part].as_ref();
            let bytes = if self.offset < part.len() {
                &part[self.offset..]
            } else if self.part + 1 < self.parts.len() {
                &self.separator[self.offset - part.len()..]
            } else {
                &[]
            };
            if bytes.is_empty() {
                self.part += 1;
                self.offset = 0;
            } else {
                let count = bytes.len().min(buffer.len() - used);
                buffer[used..used + count].copy_from_slice(&bytes[..count]);
                self.offset += count;
                used += count;
            }
        }
        Ok(used)
    }
}

enum BodyCheck {
    Any,
    Uris(UriCounter),
    Image(Option<u64>),
    Text(Vec<u8>),
}

impl BodyCheck {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), Error> {
        match self {
            Self::Uris(counter) => counter.feed(bytes),
            Self::Image(Some(remaining)) => {
                *remaining =
                    remaining
                        .checked_sub(bytes.len() as u64)
                        .ok_or(Error::InvalidInput(
                            "thumbnail byte count differs from dimensions",
                        ))?;
                Ok(())
            }
            Self::Text(tail) => {
                // Retain at most one incomplete UTF-8 character between reads.
                tail.extend_from_slice(bytes);
                match std::str::from_utf8(tail) {
                    Ok(_) => tail.clear(),
                    Err(error) if error.error_len().is_none() => {
                        tail.drain(..error.valid_up_to());
                    }
                    Err(_) => return Err(Error::InvalidInput("thumbnail text is not UTF-8")),
                }
                Ok(())
            }
            Self::Any | Self::Image(None) => Ok(()),
        }
    }

    fn finish(&mut self) -> Result<Option<u32>, Error> {
        match self {
            Self::Uris(counter) => counter.finish().map(Some),
            Self::Image(Some(0) | None) | Self::Any => Ok(None),
            Self::Image(Some(_)) => Err(Error::InvalidInput(
                "thumbnail byte count differs from dimensions",
            )),
            Self::Text(tail) if tail.is_empty() => Ok(None),
            Self::Text(_) => Err(Error::InvalidInput("thumbnail text is not UTF-8")),
        }
    }
}

impl DragDrop {
    fn ready_for_body(&self) -> Result<(), Error> {
        self.check_output()?;
        if self.drag.failed_body.is_some() {
            return Err(Error::InvalidState("report or cancel the failed data body"));
        }
        Ok(())
    }

    fn write_body(
        &mut self,
        out: &mut impl Write,
        metadata: &[&str],
        source: &mut impl Read,
        mut check: BodyCheck,
        failure: FailedBody,
    ) -> Result<Option<u32>, Error> {
        let mut incomplete = false;
        let result = self.stream_body(out, metadata, source, &mut check, &mut incomplete);
        if result.is_err() {
            self.drag.failed_body = Some(failure);
            // A thumbnail continuation would consume cancellation as image
            // data. Preserve the unfinished body instead of falsely ending it.
            self.output_failed |= incomplete;
        }
        result
    }

    fn stream_body(
        &mut self,
        out: &mut impl Write,
        metadata: &[&str],
        source: &mut impl Read,
        check: &mut BodyCheck,
        incomplete: &mut bool,
    ) -> Result<Option<u32>, Error> {
        let thumbnail = matches!(check, BodyCheck::Image(_) | BodyCheck::Text(_));
        let mut chunk_metadata = metadata.to_vec();
        if thumbnail {
            chunk_metadata.push("m=1");
        }
        let mut bytes = [0; 3072];
        let mut used = 0;
        loop {
            let count = match source.read(&mut bytes[used..]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            used += count;
            if used == bytes.len() || count == 0 {
                if used != 0 {
                    check.feed(&bytes[..used])?;
                }
                let completed_count = if count == 0 { check.finish()? } else { None };
                if used != 0 {
                    let encoded = BASE64.encode(&bytes[..used]);
                    // MIME and file blocks are independent messages. Images
                    // use one chunk chain so m=0 cannot finalize them early.
                    self.write(out, &chunk_metadata, encoded.as_bytes())?;
                    *incomplete = thumbnail;
                    used = 0;
                }
                if count == 0 {
                    self.write(out, metadata, b"")?;
                    *incomplete = false;
                    return Ok(completed_count);
                }
            }
        }
    }
}

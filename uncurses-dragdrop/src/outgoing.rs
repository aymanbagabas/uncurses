use super::*;
use stream::{BodyCheck, Separated, UriCounter};

impl DragDrop {
    /// Prepare a URI offer and pre-send its complete URI list.
    ///
    /// Supply directory URLs with a trailing `/`. The controller transmits
    /// URLs as supplied and does not inspect or normalize filesystem paths.
    pub fn offer_files(
        &mut self,
        out: &mut impl Write,
        uris: &[&str],
        operations: Operations,
    ) -> Result<(), Error> {
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
        self.send_from(out, Entry::UriList, &mut Separated::new(uris, b"\r\n"))
    }

    /// Send a complete MIME body or regular file, including its end marker.
    ///
    /// During preparation, MIME targets supply initial data. During an active
    /// drag, MIME targets answer [`Event::DataRequested`]. File targets answer
    /// [`Event::EntryRequested`] or supply pending directory children.
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
            Entry::UriEntry(index) if *index == root && self.drag.children.is_empty() => {}
            Entry::DirectoryEntry { handle, index }
                if self.drag.children.front() == Some(entry)
                    && self.drag.directories.get(handle) == Some(&root) =>
            {
                metadata.push(format!("Y={handle}"));
                metadata.push(format!("y={index}"));
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

    fn finish_file(&mut self, entry: &Entry, directory: Option<(i64, u32)>) {
        if matches!(entry, Entry::DirectoryEntry { .. }) {
            self.drag.children.pop_front();
        }
        if let Some((handle, count)) = directory {
            let root = *self
                .drag
                .file_requests
                .front()
                .expect("file_target verified request");
            self.drag.directories.insert(handle, root);
            self.drag
                .children
                .extend((1..=count).map(|index| Entry::DirectoryEntry { handle, index }));
        }
        if self.drag.children.is_empty() {
            self.drag.file_requests.pop_front();
            self.drag.directories.clear();
        }
    }

    /// Send the bytes of a symbolic link target for a pending file entry.
    pub fn send_symlink(
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

    /// Send a directory listing and return its allocated wire handle.
    ///
    /// Names can contain arbitrary bytes except NUL and must be nonempty.
    /// Send every child with [`Entry::DirectoryEntry`], in list order.
    /// Send siblings before descendants, then proceed to the next root request.
    /// An empty directory completes immediately.
    pub fn send_directory(
        &mut self,
        out: &mut impl Write,
        entry: impl Into<Entry>,
        names: &[&[u8]],
    ) -> Result<i64, Error> {
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
        self.finish_file(&entry, Some((i64::from(handle), count)));
        Ok(i64::from(handle))
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

use crate::protocol::{Message, UriCounter};
use crate::types::Identity;
use crate::{Directory, DragDrop, Entry, EntryKind, Error, Event};
use std::{collections::HashMap, io::Write};

#[derive(Debug, Default)]
pub(crate) struct Received {
    pub(crate) scope: Identity,
    pub(crate) uris: Option<u32>,
    pub(crate) directories: HashMap<i64, (Directory, u32)>,
}

#[derive(Debug)]
pub(crate) struct Incoming {
    entry: Entry,
    x: u32,
    y: u32,
    parent: i64,
    list: bool,
    kind: Option<EntryKind>,
    remote: Option<bool>,
    records: Records,
    uris: UriCounter,
}

impl Incoming {
    pub(crate) fn matches(&self, message: &Message) -> Result<(), Error> {
        if message.number("x")? != i64::from(self.x)
            || message.number("y")? != i64::from(self.y)
            || message.number("Y")? != self.parent
        {
            return Err(Error::Protocol(
                "response target differs from request".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct Records {
    pending: Vec<u8>,
    count: u32,
}

impl Records {
    fn feed(
        &mut self,
        bytes: &[u8],
        end: bool,
        scope: Identity,
        directory: Option<i64>,
        events: &mut Vec<Event>,
    ) -> Result<(), Error> {
        for &byte in bytes {
            let separator = if directory.is_some() {
                byte == 0
            } else {
                byte == b'\r' || byte == b'\n'
            };
            if separator {
                self.record(scope, directory, events)?;
            } else {
                if directory.is_none() && byte == 0 {
                    return Err(Error::Protocol("NUL in URI list".into()));
                }
                if self.pending.len() == 64 * 1024 {
                    return Err(Error::Protocol("listing record exceeds 64 KiB".into()));
                }
                self.pending.push(byte);
            }
        }
        if end && !self.pending.is_empty() {
            self.record(scope, directory, events)?;
        }
        Ok(())
    }

    fn record(
        &mut self,
        scope: Identity,
        directory: Option<i64>,
        events: &mut Vec<Event>,
    ) -> Result<(), Error> {
        let name = std::mem::take(&mut self.pending);
        if directory.is_none()
            && (name.first() == Some(&b'#') || name.iter().all(|b| matches!(b, b' ' | b'\t')))
        {
            return Ok(());
        }
        if name.is_empty() {
            return Err(Error::Protocol("empty directory name".into()));
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| Error::Protocol("too many listed entries".into()))?;
        let entry = Entry::file(scope, directory.unwrap_or(0), self.count);
        events.push(Event::Listed { entry, name });
        Ok(())
    }
}

fn valid_handle(handle: i64) -> bool {
    (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&handle) && handle != 0 && handle != 1
}

impl DragDrop {
    /// Request a MIME body, root URI, or directory child after an actual drop.
    ///
    /// Only one request can be active. Read the URI list before requesting
    /// its entries. Wait for [`Event::DataEnd`] before the next request.
    /// A raw [`Entry::UriList`] request emits the original [`Event::Data`] bytes.
    pub fn request(&mut self, out: &mut impl Write, entry: impl Into<Entry>) -> Result<(), Error> {
        self.request_entry(out, entry.into(), false)
    }

    /// Request the URI list as streamed [`Event::Listed`] notifications.
    ///
    /// Names retain the original URI bytes, without line separators.
    /// Comments and empty lines do not consume entry indices.
    /// Each URI or directory name is limited to 64 KiB across data messages.
    pub fn list(&mut self, out: &mut impl Write) -> Result<(), Error> {
        self.request_entry(out, Entry::UriList, true)
    }

    fn request_entry(
        &mut self,
        out: &mut impl Write,
        entry: Entry,
        list: bool,
    ) -> Result<(), Error> {
        let offer = self
            .dropped
            .as_ref()
            .ok_or(Error::InvalidState("request requires an actual drop"))?;
        if self.request.is_some() {
            return Err(Error::InvalidState("an incoming request is still active"));
        }
        let mime_index = |mime: &str| -> Result<u32, Error> {
            let index = offer
                .mime_types
                .iter()
                .position(|m| m == mime)
                .ok_or(Error::InvalidInput("MIME type is not offered"))?;
            u32::try_from(index + 1).map_err(|_| Error::InvalidInput("too many MIME types"))
        };
        let (x, y, parent) = match &entry {
            Entry::File(file) if file.parent == 0 => {
                if file.scope != self.received.scope
                    || file.index == 0
                    || self.received.uris.is_none_or(|count| file.index > count)
                {
                    return Err(Error::InvalidInput("URI entry is not in a completed list"));
                }
                (mime_index("text/uri-list")?, file.index, 0)
            }
            Entry::File(file) => {
                if !valid_handle(file.parent)
                    || file.index == 0
                    || self.received.directories.get(&file.parent).is_none_or(
                        |(directory, count)| directory.scope != file.scope || file.index > *count,
                    )
                {
                    return Err(Error::InvalidInput(
                        "directory entry is not in an open directory",
                    ));
                }
                (file.index, 0, file.parent)
            }
            _ => (mime_index(entry.mime().expect("MIME entry"))?, 0, 0),
        };
        let mut metadata = vec!["t=r".to_owned(), format!("x={x}")];
        if y != 0 {
            metadata.push(format!("y={y}"));
        }
        if parent != 0 {
            metadata.push(format!("Y={parent}"));
        }
        self.write(
            out,
            &metadata.iter().map(String::as_str).collect::<Vec<_>>(),
            b"",
        )?;
        if entry.is_uri_list() {
            self.received.scope = Identity::default();
            self.received.uris = None;
        }
        self.request = Some(Incoming {
            entry,
            x,
            y,
            parent,
            list,
            kind: None,
            remote: None,
            records: Records::default(),
            uris: UriCounter::default(),
        });
        Ok(())
    }

    /// Release an open incoming directory and invalidate its child entries.
    ///
    /// Wait for the current request to end first. Children with their own
    /// directory handles remain open until explicitly closed.
    pub fn close_directory(
        &mut self,
        out: &mut impl Write,
        directory: Directory,
    ) -> Result<(), Error> {
        if self.request.is_some() {
            return Err(Error::InvalidState("an incoming request is still active"));
        }
        let handle = directory.handle;
        if self
            .received
            .directories
            .get(&handle)
            .map(|(open, _)| *open)
            != Some(directory)
        {
            return Err(Error::InvalidInput("directory handle is not open"));
        }
        self.write(out, &["t=r", &format!("Y={handle}")], b"")?;
        self.received.directories.remove(&handle);
        Ok(())
    }

    pub(crate) fn incoming_data(
        &mut self,
        message: &mut Message,
        final_chunk: bool,
        empty: bool,
    ) -> Result<Vec<Event>, Error> {
        let Some(request) = &mut self.request else {
            // Canceled responses can remain in flight.
            message.payload.clear();
            return Ok(Vec::new());
        };
        request.matches(message)?;
        let tag = message.number("X")?;
        let mut events = Vec::new();
        if request.entry.is_uri_list() {
            let remote = match tag {
                0 => false,
                1 => true,
                _ => return Err(Error::Protocol("invalid URI list remote marker".into())),
            };
            if let Some(previous) = request.remote {
                if previous != remote {
                    return Err(Error::Protocol("URI list remote marker changed".into()));
                }
            } else {
                request.remote = Some(remote);
                events.push(Event::UriList { remote });
            }
        } else if request.entry.mime().is_none() {
            let kind = match tag {
                0 => EntryKind::File,
                1 => EntryKind::Symlink,
                handle => EntryKind::Directory(Directory {
                    scope: match request.kind {
                        Some(EntryKind::Directory(directory)) => directory.scope,
                        _ => Identity::default(),
                    },
                    handle,
                }),
            };
            if let Some(previous) = request.kind {
                if previous != kind {
                    return Err(Error::Protocol("entry kind changed during its body".into()));
                }
            } else {
                if let EntryKind::Directory(directory) = kind {
                    let handle = directory.handle;
                    if self.received.directories.contains_key(&handle) {
                        return Err(Error::Protocol("directory handle is already open".into()));
                    }
                    self.received.directories.insert(handle, (directory, 0));
                }
                request.kind = Some(kind);
                events.push(Event::Entry(kind));
            }
        } else if tag != 0 {
            return Err(Error::Protocol("unexpected MIME entry kind".into()));
        }
        let bytes = message.decode_data(final_chunk)?;
        let end = final_chunk && empty;
        if let Some(EntryKind::Directory(directory)) = request.kind {
            let handle = directory.handle;
            request
                .records
                .feed(&bytes, end, directory.scope, Some(handle), &mut events)?;
            if end {
                self.received
                    .directories
                    .insert(handle, (directory, request.records.count));
            }
        } else {
            if request.entry.is_uri_list() {
                if request.list {
                    request
                        .records
                        .feed(&bytes, end, self.received.scope, None, &mut events)?;
                    if end {
                        self.received.uris = Some(request.records.count);
                    }
                } else {
                    request
                        .uris
                        .feed(&bytes)
                        .map_err(|error| Error::Protocol(error.to_string()))?;
                    if end {
                        self.received.uris = Some(
                            request
                                .uris
                                .finish()
                                .map_err(|error| Error::Protocol(error.to_string()))?,
                        );
                    }
                }
            }
            if !request.list && !bytes.is_empty() {
                events.push(Event::Data(bytes));
            }
        }
        if end {
            self.request = None;
            events.push(Event::DataEnd);
        }
        Ok(events)
    }
}

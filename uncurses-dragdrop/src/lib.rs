//! Event-driven Kitty drag and drop.
//!
//! A [`DragDrop`] controller lives alongside the application's terminal event
//! loop. It handles MIME indices, protocol messages, base64, and wire chunks.
//! The application owns its data and decides when to accept or complete a drop.
//!
//! Feed raw uncurses events into [`DragDrop::handle_event`]. Incoming content
//! arrives as [`Event::Data`] followed by [`Event::DataEnd`]. Write those bytes
//! to a file, process them directly, or collect them in a buffer.
//! [`DragDrop::send`] and [`DragDrop::send_from`] transmit complete bodies.
//!
//! Confirm terminal support with uncurses' capability query before registration.
//! Output methods write without flushing. Flush the terminal writer after each
//! batch. Call [`DragDrop::close`] before terminal teardown.
//!
//! ```
//! use uncurses_dragdrop::{DragDrop, DragDropOptions};
//!
//! let mut controller = DragDrop::new(DragDropOptions::default());
//! let mut output = Vec::new();
//! controller.enable_drops(&mut output, &["text/plain"])?;
//! controller.enable_drags(&mut output)?;
//! controller.close(&mut output)?;
//! # Ok::<(), uncurses_dragdrop::Error>(())
//! ```

mod incoming;
mod machine_id;
mod outgoing;
mod protocol;
mod stream;
mod types;

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};

use incoming::{Incoming, Received};
use protocol::{Message, error_payload, join_mimes, mime_list, value};
use uncurses::ansi::drag_drop::write_drag_drop;

pub use types::{
    DragDropOptions, DropOffer, Entry, EntryKind, Error, Event, MachineId, Operation, Operations,
    Position, RemoteError, ThumbnailFormat,
};

#[derive(Debug, Default, PartialEq, Eq)]
enum DragPhase {
    #[default]
    Idle,
    Requested,
    Preparing,
    Starting,
    Active,
}

#[derive(Debug, Default)]
struct Drag {
    phase: DragPhase,
    mimes: Vec<String>,
    requests: Vec<usize>,
    failed_body: Option<FailedBody>,
    uri_count: Option<u32>,
    file_requests: VecDeque<u32>,
    children: VecDeque<Entry>,
    directories: HashMap<i64, u32>,
    next_handle: u32,
    thumbnails: u32,
}

#[derive(Debug, PartialEq, Eq)]
enum FailedBody {
    Mime(usize),
    Global,
}

/// A reusable controller for one logical terminal endpoint.
///
/// Receiving and offering have separate registrations and exchange state.
/// Both use the fixed identities supplied at construction. Rust destruction
/// performs no I/O; call [`Self::close`] explicitly.
#[derive(Debug, Default)]
pub struct DragDrop {
    options: DragDropOptions,
    drops_enabled: bool,
    drags_enabled: bool,
    hover: Option<DropOffer>,
    dropped: Option<DropOffer>,
    request: Option<Incoming>,
    received: Received,
    drag: Drag,
    continuation: Option<Message>,
    output_failed: bool,
}

impl DragDrop {
    /// Construct a controller with fixed endpoint identities.
    pub fn new(options: DragDropOptions) -> Self {
        Self {
            options,
            ..Self::default()
        }
    }

    /// Register MIME types for incoming notifications.
    ///
    /// An empty list registers general interest. Registration is separate from
    /// accepting a particular offer. Re-register only after disabling drops.
    pub fn enable_drops(&mut self, out: &mut impl Write, mimes: &[&str]) -> Result<(), Error> {
        let payload = join_mimes(mimes, true)?;
        if self.drops_enabled {
            return Err(Error::InvalidState("drops are already enabled"));
        }
        self.write(out, &["t=a"], payload.as_bytes())?;
        self.drops_enabled = true;
        if let Some(id) = self.options.machine_id.clone() {
            self.write(out, &["t=a", "x=1"], id.as_str().as_bytes())?;
        }
        Ok(())
    }

    /// Register interest in terminal drag gestures.
    pub fn enable_drags(&mut self, out: &mut impl Write) -> Result<(), Error> {
        if self.drags_enabled {
            return Err(Error::InvalidState("drags are already enabled"));
        }
        self.register_drags(out)?;
        self.drags_enabled = true;
        Ok(())
    }

    fn register_drags(&mut self, out: &mut impl Write) -> Result<(), Error> {
        let id = self
            .options
            .machine_id
            .as_ref()
            .map(|id| id.as_str().to_owned())
            .unwrap_or_default();
        self.write(out, &["t=o", "x=1"], id.as_bytes())
    }

    /// Accept a potential drop, with MIME types in preference order.
    pub fn accept(
        &mut self,
        out: &mut impl Write,
        mimes: &[&str],
        operation: Operation,
    ) -> Result<(), Error> {
        let payload = join_mimes(mimes, true)?;
        let offer = self
            .hover
            .as_ref()
            .ok_or(Error::InvalidState("accept requires a hover offer"))?;
        Self::check_operation(offer, operation)?;
        if mimes
            .iter()
            .any(|mime| !offer.mime_types.iter().any(|offered| offered == mime))
        {
            return Err(Error::InvalidInput("MIME type is not offered"));
        }
        let operation = format!("o={}", operation.flag().bits());
        self.write(out, &["t=m", &operation], payload.as_bytes())
    }

    /// Reject a potential drop.
    pub fn reject(&mut self, out: &mut impl Write) -> Result<(), Error> {
        if self.hover.is_none() {
            return Err(Error::InvalidState("reject requires a hover offer"));
        }
        self.write(out, &["t=m", "o=0"], b"")
    }

    /// Report application success after consuming the desired content.
    ///
    /// Copy promised files before this call; their lifetime can end with the
    /// drop. The application chooses the final operation.
    pub fn complete_drop(
        &mut self,
        out: &mut impl Write,
        operation: Operation,
    ) -> Result<(), Error> {
        let offer = self
            .dropped
            .as_ref()
            .ok_or(Error::InvalidState("there is no active drop"))?;
        Self::check_operation(offer, operation)?;
        if self.request.is_some() {
            return Err(Error::InvalidState("incoming data is still pending"));
        }
        self.write(
            out,
            &["t=r", &format!("o={}", operation.flag().bits())],
            b"",
        )?;
        self.clear_drop();
        Ok(())
    }

    /// Cancel an actual drop. Calling this while idle has no effect.
    pub fn cancel_drop(&mut self, out: &mut impl Write) -> Result<(), Error> {
        if self.dropped.is_some() {
            self.write(out, &["t=r", "o=0"], b"")?;
            self.clear_drop();
        }
        Ok(())
    }

    fn clear_drop(&mut self) {
        self.dropped = None;
        self.request = None;
        self.received = Received::default();
        self.discard_continuation(&["m", "M", "r", "R"]);
    }

    /// Prepare a drag in response to [`Event::DragRequested`].
    ///
    /// Supply initial data with [`Self::send`] or [`Self::send_from`], then call
    /// [`Self::start_drag`]. Small text and URI bodies should be pre-sent for
    /// native application interoperability.
    pub fn offer(
        &mut self,
        out: &mut impl Write,
        mimes: &[&str],
        operations: Operations,
    ) -> Result<(), Error> {
        let payload = join_mimes(mimes, false)?;
        if operations.is_empty() || Operations::from_bits(operations.bits()).is_none() {
            return Err(Error::InvalidInput("expected copy or move operations"));
        }
        if self.drag.phase != DragPhase::Requested {
            return Err(Error::InvalidState("offer requires a drag gesture"));
        }
        self.write(
            out,
            &["t=o", &format!("o={}", operations.bits())],
            payload.as_bytes(),
        )?;
        self.drag.mimes = mimes.iter().map(|mime| (*mime).to_owned()).collect();
        self.drag.phase = DragPhase::Preparing;
        Ok(())
    }

    fn data_target(&self, mime: &str) -> Result<(usize, bool), Error> {
        let initial = match self.drag.phase {
            DragPhase::Preparing => true,
            DragPhase::Active => false,
            _ => return Err(Error::InvalidState("drag is not ready for data")),
        };
        let index = self
            .drag
            .mimes
            .iter()
            .position(|offered| offered == mime)
            .ok_or(Error::InvalidInput("MIME type is not offered"))?;
        u32::try_from(index).map_err(|_| Error::InvalidInput("too many MIME types"))?;
        if !initial && !self.drag.requests.contains(&index) {
            return Err(Error::InvalidState("MIME data was not requested"));
        }
        Ok((index, initial))
    }

    /// Request startup after preparing the offer and any initial data.
    /// An offer with `text/uri-list` requires its complete initial body.
    /// Wait for [`Event::DragStarted`] before answering later data requests.
    pub fn start_drag(&mut self, out: &mut impl Write) -> Result<(), Error> {
        if self.drag.phase != DragPhase::Preparing {
            return Err(Error::InvalidState("start requires a prepared offer"));
        }
        if self.drag.failed_body.is_some() {
            return Err(Error::InvalidState("cancel the failed initial data body"));
        }
        if self.drag.mimes.iter().any(|mime| mime == "text/uri-list")
            && self.drag.uri_count.is_none()
        {
            return Err(Error::InvalidState(
                "pre-send the URI list before starting the drag",
            ));
        }
        self.write(out, &["t=P", "x=-1"], b"")?;
        self.drag.phase = DragPhase::Starting;
        Ok(())
    }

    /// Report failure while answering one outgoing MIME request.
    /// Other requests and the drag remain active.
    /// Cancel the drag instead if initial data cannot be prepared.
    pub fn fail_data(
        &mut self,
        out: &mut impl Write,
        mime: &str,
        error: RemoteError,
    ) -> Result<(), Error> {
        let payload = error_payload(&error)?;
        let (index, initial) = self.data_target(mime)?;
        if initial {
            return Err(Error::InvalidState("cancel an incomplete initial offer"));
        }
        if self
            .drag
            .failed_body
            .as_ref()
            .is_some_and(|failed| *failed != FailedBody::Mime(index))
        {
            return Err(Error::InvalidState("report the failed MIME body first"));
        }
        self.write(out, &["t=E", &format!("y={index}")], payload.as_bytes())?;
        let pending = self
            .drag
            .requests
            .iter()
            .position(|requested| *requested == index)
            .expect("data_target verified the pending request");
        self.drag.requests.remove(pending);
        self.drag.failed_body = None;
        Ok(())
    }

    /// Cancel an outgoing exchange while preserving gesture registration.
    ///
    /// During preparation, unregister and register the offering role to release
    /// its buffers. After the startup request, send cancellation in command
    /// order, even if the application has not consumed the acknowledgment.
    pub fn cancel_drag(&mut self, out: &mut impl Write) -> Result<(), Error> {
        match self.drag.phase {
            DragPhase::Preparing => {
                self.write(out, &["t=o", "x=2"], b"")?;
                self.register_drags(out)?;
            }
            DragPhase::Starting | DragPhase::Active => self.write(out, &["t=E", "y=-1"], b"")?,
            DragPhase::Idle | DragPhase::Requested => {}
        }
        self.drag = Drag::default();
        self.discard_continuation(&["o", "e", "E", "k"]);
        Ok(())
    }

    /// Cancel an active drop and unregister the receiving role.
    pub fn disable_drops(&mut self, out: &mut impl Write) -> Result<(), Error> {
        if !self.drops_enabled {
            return Ok(());
        }
        let cancel = self.cancel_drop(out);
        let unregister = self.write(out, &["t=A"], b"");
        if unregister.is_ok() {
            self.drops_enabled = false;
            self.hover = None;
            self.clear_drop();
        }
        cancel.and(unregister)
    }

    /// Cancel an active drag and unregister the offering role.
    pub fn disable_drags(&mut self, out: &mut impl Write) -> Result<(), Error> {
        if !self.drags_enabled {
            return Ok(());
        }
        let cancel = if matches!(self.drag.phase, DragPhase::Starting | DragPhase::Active) {
            self.write(out, &["t=E", "y=-1"], b"")
        } else {
            Ok(())
        };
        let unregister = self.write(out, &["t=o", "x=2"], b"");
        if unregister.is_ok() {
            self.drags_enabled = false;
            self.drag = Drag::default();
            self.discard_continuation(&["o", "e", "E", "k"]);
        }
        cancel.and(unregister)
    }

    /// Attempt cleanup for both roles, returning the first error.
    ///
    /// Flush afterward, then restore the terminal. A poisoned output stream
    /// requires terminal recovery; this method does not retry partial messages.
    pub fn close(&mut self, out: &mut impl Write) -> Result<(), Error> {
        let drops = self.disable_drops(out);
        let drags = self.disable_drags(out);
        drops.and(drags)
    }

    fn check_operation(offer: &DropOffer, operation: Operation) -> Result<(), Error> {
        if !offer.operations.contains(operation.flag()) {
            return Err(Error::InvalidInput("operation is not permitted"));
        }
        Ok(())
    }

    fn discard_continuation(&mut self, kinds: &[&str]) {
        if let Some(message) = &mut self.continuation
            && message.value("t").is_some_and(|kind| kinds.contains(&kind))
        {
            // Drain the remaining frames without reviving an old exchange
            // if the application registers the role again before they arrive.
            message.discarded = true;
            message.payload.clear();
        }
    }

    fn check_output(&self) -> Result<(), Error> {
        if self.output_failed {
            Err(Error::OutputFailed)
        } else {
            Ok(())
        }
    }

    fn write(
        &mut self,
        out: &mut impl Write,
        metadata: &[&str],
        payload: &[u8],
    ) -> Result<(), Error> {
        self.check_output()?;
        let id = self.options.routing_id.map(|id| format!("i={id}"));
        let mut options = metadata.to_vec();
        if let Some(id) = &id {
            options.push(id);
        }
        let result = if payload.is_empty() {
            write_drag_drop(out, &options, b"")
        } else {
            let mut chunks = payload.chunks(4096).peekable();
            let mut result = Ok(());
            while let Some(chunk) = chunks.next() {
                let more = chunks.peek().is_some();
                if more {
                    options.push("m=1");
                }
                result = write_drag_drop(out, &options, chunk);
                if more {
                    options.pop();
                }
                if result.is_err() {
                    break;
                }
            }
            result
        };
        if let Err(error) = result {
            self.output_failed = true;
            return Err(error.into());
        }
        Ok(())
    }

    /// Parse one raw event into application notifications.
    ///
    /// Other event types, other routing IDs, and capability query replies are
    /// ignored. Queries can interleave with a chunk chain. Incoming bytes are
    /// decoded incrementally; one input can produce final data and `DataEnd`.
    ///
    /// Protocol errors require application cleanup. File content and names
    /// remain bytes; the controller does not access the filesystem.
    pub fn handle_event(&mut self, event: &uncurses::event::Event) -> Result<Vec<Event>, Error> {
        let (metadata, payload) = match event {
            uncurses::event::Event::DragDrop { options, payload } => (options, payload),
            uncurses::event::Event::UnknownOsc(bytes) if bytes.starts_with(b"72;") => {
                return Err(Error::Protocol("malformed OSC 72 message".into()));
            }
            _ => return Ok(Vec::new()),
        };
        let id = value(metadata, "i")
            .unwrap_or("0")
            .parse::<u32>()
            .map_err(|_| Error::Protocol("invalid routing ID".into()))?;
        if id != self.options.routing_id.unwrap_or(0) {
            return Ok(Vec::new());
        }
        if matches!(value(metadata, "t"), Some("q" | "Q")) {
            return Ok(Vec::new());
        }
        let more = match value(metadata, "m").unwrap_or("0") {
            "0" => false,
            "1" => true,
            _ => return Err(Error::Protocol("invalid chunk flag".into())),
        };
        let mut message = self.continuation.take().unwrap_or_else(|| Message {
            metadata: metadata.clone(),
            payload: Vec::new(),
            discarded: match value(metadata, "t") {
                Some("m" | "M" | "r" | "R") => !self.drops_enabled,
                Some("o" | "e" | "E" | "k") => !self.drags_enabled,
                _ => false,
            },
        });
        if !message.discarded {
            message.payload.extend_from_slice(payload);
        }
        let result = if message.discarded {
            Ok(Vec::new())
        } else if message.value("t") == Some("r") {
            self.incoming_data(&mut message, !more, payload.is_empty())
        } else if more {
            Ok(Vec::new())
        } else {
            self.message(&message)
        };
        if result.is_err() {
            if matches!(message.value("t"), Some("r" | "R")) {
                self.request = None;
                self.received = Received::default();
            }
            message.discarded = true;
            message.payload.clear();
        }
        if more {
            self.continuation = Some(message);
        }
        result
    }

    fn message(&mut self, message: &Message) -> Result<Vec<Event>, Error> {
        let mut events = Vec::new();
        let event = match message.value("t") {
            Some("m" | "M") if self.drops_enabled => {
                if self.dropped.is_some() {
                    self.clear_drop();
                    events.push(Event::DropCanceled);
                }
                let actual = message.value("t") == Some("M");
                if !actual && message.number("x")? == -1 && message.number("y")? == -1 {
                    self.hover = None;
                    events.push(Event::Leave);
                    return Ok(events);
                }
                let mimes = if !actual && message.payload.is_empty() {
                    self.hover
                        .as_ref()
                        .map(|offer| offer.mime_types.clone())
                        .ok_or_else(|| Error::Protocol("initial hover omitted MIME types".into()))?
                } else {
                    mime_list(&message.payload)?
                };
                let offer = DropOffer {
                    position: message.position()?,
                    mime_types: mimes,
                    operations: message.operations()?,
                };
                if actual {
                    self.hover = None;
                    self.dropped = Some(offer.clone());
                    Event::Drop(offer)
                } else {
                    self.hover = Some(offer.clone());
                    Event::Hover(offer)
                }
            }
            Some("R") if self.drops_enabled => {
                let error = message.remote_error()?;
                if let Some(request) = &self.request {
                    request.matches(message)?;
                }
                self.clear_drop();
                Event::DropFailed(error)
            }
            Some("o") if self.drags_enabled => {
                if !matches!(self.drag.phase, DragPhase::Idle | DragPhase::Requested) {
                    return Err(Error::Protocol(
                        "another outgoing drag is still active".into(),
                    ));
                }
                let position = message.position()?;
                self.drag.phase = DragPhase::Requested;
                Event::DragRequested(position)
            }
            Some("E") if self.drags_enabled && self.drag.phase != DragPhase::Idle => {
                if message.payload == b"OK" {
                    if self.drag.phase != DragPhase::Starting {
                        return Err(Error::Protocol("unexpected startup acknowledgment".into()));
                    }
                    self.drag.phase = DragPhase::Active;
                    Event::DragStarted
                } else {
                    let error = message.remote_error()?;
                    self.drag = Drag::default();
                    Event::DragFailed(error)
                }
            }
            Some("e") if self.drags_enabled && self.drag.phase == DragPhase::Active => {
                match message.number("x")? {
                    1 | 5 => {
                        let index = message.unsigned("y")? as usize;
                        let mime = self.drag.mimes.get(index).ok_or_else(|| {
                            Error::Protocol("requested MIME index is out of range".into())
                        })?;
                        let mime_type = mime.clone();
                        if message.number("x")? == 5 {
                            self.drag.requests.push(index);
                            Event::DataRequested { mime_type }
                        } else {
                            Event::DragAccepted { mime_type }
                        }
                    }
                    2 => Event::DragActionChanged {
                        operation: match message.number("o")? {
                            0 => None,
                            1 => Some(Operation::Copy),
                            2 => Some(Operation::Move),
                            _ => return Err(Error::Protocol("invalid proposed operation".into())),
                        },
                    },
                    3 => Event::DragDropped,
                    4 => {
                        let canceled = match message.number("y")? {
                            0 => false,
                            1 => true,
                            _ => return Err(Error::Protocol("invalid completion status".into())),
                        };
                        self.drag = Drag::default();
                        Event::DragFinished { canceled }
                    }
                    _ => return Ok(Vec::new()),
                }
            }
            Some("k") if self.drags_enabled && self.drag.phase == DragPhase::Active => {
                let index = message.unsigned("x")?;
                if index == 0
                    || self.drag.uri_count.is_none_or(|count| index > count)
                    || message.number("y")? != 0
                    || message.number("Y")? != 0
                    || !message.payload.is_empty()
                {
                    return Err(Error::Protocol("invalid URI entry request".into()));
                }
                self.drag.file_requests.push_back(index);
                Event::EntryRequested {
                    entry: Entry::UriEntry(index),
                }
            }
            _ => return Ok(Vec::new()),
        };
        events.push(event);
        Ok(events)
    }
}

#[cfg(test)]
mod tests;

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

mod protocol;
mod types;

use std::io::{self, Read, Write};

use base64::Engine;
use protocol::{BASE64, Message, error_payload, join_mimes, mime_list, value};
use uncurses::ansi::drag_drop::write_drag_drop;

pub use types::{
    DragDropOptions, DropOffer, Error, Event, MachineId, Operation, Operations, Position,
    RemoteError,
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
    failed_body: Option<usize>,
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
    request: Option<usize>,
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
        let payload = join_mimes(mimes, false)?;
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

    /// Request a MIME body after an actual drop.
    ///
    /// Only one incoming body is active at a time. After [`Event::DataEnd`],
    /// request another type or complete the drop.
    pub fn request(&mut self, out: &mut impl Write, mime: &str) -> Result<(), Error> {
        let offer = self
            .dropped
            .as_ref()
            .ok_or(Error::InvalidState("request requires an actual drop"))?;
        if self.request.is_some() {
            return Err(Error::InvalidState("an incoming request is still active"));
        }
        let index = offer
            .mime_types
            .iter()
            .position(|offered| offered == mime)
            .ok_or(Error::InvalidInput("MIME type is not offered"))?;
        let wire_index =
            u32::try_from(index + 1).map_err(|_| Error::InvalidInput("too many MIME types"))?;
        self.write(out, &["t=r", &format!("x={wire_index}")], b"")?;
        self.request = Some(index);
        Ok(())
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

    /// Send a complete MIME body, including its protocol end marker.
    ///
    /// Before startup this supplies initial data. During an active drag it
    /// answers a [`Event::DataRequested`] notification.
    pub fn send(
        &mut self,
        out: &mut impl Write,
        mime: &str,
        mut bytes: &[u8],
    ) -> Result<(), Error> {
        self.send_from(out, mime, &mut bytes)
    }

    /// Stream a complete body from a reader with bounded data buffers.
    ///
    /// This synchronous call reads until EOF. Slow reads or writes block the
    /// caller. An interrupted read is retried. Other read errors leave the body
    /// unfinished; call [`Self::fail_data`] or [`Self::cancel_drag`].
    /// Output errors poison the controller because terminal state is uncertain.
    pub fn send_from(
        &mut self,
        out: &mut impl Write,
        mime: &str,
        source: &mut impl Read,
    ) -> Result<(), Error> {
        self.check_output()?;
        if self.drag.failed_body.is_some() {
            return Err(Error::InvalidState("report or cancel the failed data body"));
        }
        let (index, initial) = self.data_target(mime)?;
        let kind = if initial { "t=p" } else { "t=e" };
        let index_key = if initial { "x" } else { "y" };
        let index_option = format!("{index_key}={index}");
        let mut bytes = [0; 3072];
        let mut used = 0;
        loop {
            let count = match source.read(&mut bytes[used..]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.drag.failed_body = Some(index);
                    return Err(error.into());
                }
                Ok(count) => count,
            };
            used += count;
            if used == bytes.len() || count == 0 {
                if used != 0 {
                    let encoded = BASE64.encode(&bytes[..used]);
                    self.write(out, &[kind, &index_option], encoded.as_bytes())?;
                    used = 0;
                }
                if count == 0 {
                    break;
                }
            }
        }
        self.write(out, &[kind, &index_option], b"")?;
        if !initial {
            let pending = self
                .drag
                .requests
                .iter()
                .position(|requested| *requested == index)
                .expect("data_target verified the pending request");
            self.drag.requests.remove(pending);
        }
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
    /// Wait for [`Event::DragStarted`] before answering later data requests.
    pub fn start_drag(&mut self, out: &mut impl Write) -> Result<(), Error> {
        if self.drag.phase != DragPhase::Preparing {
            return Err(Error::InvalidState("start requires a prepared offer"));
        }
        if self.drag.failed_body.is_some() {
            return Err(Error::InvalidState("cancel the failed initial data body"));
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
        if self.drag.failed_body.is_some_and(|failed| failed != index) {
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
    /// Protocol and unsupported-feature errors require application cleanup.
    /// Remote URI responses and remote file requests return
    /// [`Error::UnsupportedRemoteFiles`], rather than treating remote paths as
    /// local files.
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
        let events = if message.discarded {
            Vec::new()
        } else if message.value("t") == Some("r") {
            self.incoming_data(&mut message, !more, payload.is_empty())?
        } else if more {
            Vec::new()
        } else {
            self.message(&message)?
        };
        if more {
            self.continuation = Some(message);
        }
        Ok(events)
    }

    fn incoming_data(
        &mut self,
        message: &mut Message,
        final_chunk: bool,
        empty: bool,
    ) -> Result<Vec<Event>, Error> {
        let Some(request) = self.request else {
            // Data can already be in flight when the application cancels.
            message.payload.clear();
            return Ok(Vec::new());
        };
        if message.number("x")? != (request + 1) as i64 {
            return Err(Error::Protocol(
                "response MIME index differs from request".into(),
            ));
        }
        if message.number("y")? != 0 || message.number("Y")? != 0 || message.number("X")? != 0 {
            return Err(Error::UnsupportedRemoteFiles);
        }
        let bytes = message.decode_data(final_chunk)?;
        let mut events = Vec::new();
        if !bytes.is_empty() {
            events.push(Event::Data(bytes));
        }
        if final_chunk && empty {
            self.request = None;
            events.push(Event::DataEnd);
        }
        Ok(events)
    }

    fn message(&mut self, message: &Message) -> Result<Vec<Event>, Error> {
        let event = match message.value("t") {
            Some("m" | "M") if self.drops_enabled => {
                if self.dropped.is_some() {
                    return Err(Error::Protocol("another drop is still active".into()));
                }
                let actual = message.value("t") == Some("M");
                if !actual && message.number("x")? == -1 && message.number("y")? == -1 {
                    self.hover = None;
                    return Ok(vec![Event::Leave]);
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
                if let Some(request) = self.request
                    && message.number("x")? != (request + 1) as i64
                {
                    return Err(Error::Protocol(
                        "error MIME index differs from request".into(),
                    ));
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
            Some("k") if self.drags_enabled => return Err(Error::UnsupportedRemoteFiles),
            _ => return Ok(Vec::new()),
        };
        Ok(vec![event])
    }
}

#[cfg(test)]
mod tests;

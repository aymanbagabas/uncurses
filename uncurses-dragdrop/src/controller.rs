use crate::incoming::{Incoming, Received};
use crate::protocol::{Message, error_payload, join_mimes, mime_list, value};
use crate::types::Identity;
use crate::{DragDropOptions, DropOffer, Entry, Error, Event, Operation, Operations, RemoteError};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
};
use uncurses::ansi::drag_drop::write_drag_drop;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) enum DragPhase {
    #[default]
    Idle,
    Requested,
    Preparing,
    Starting,
    Active,
}

#[derive(Debug, Default)]
pub(crate) struct Drag {
    pub(crate) scope: Identity,
    pub(crate) phase: DragPhase,
    pub(crate) mimes: Vec<String>,
    pub(crate) requests: Vec<usize>,
    pub(crate) failed_body: Option<FailedBody>,
    pub(crate) uri_count: Option<u32>,
    pub(crate) file_requests: VecDeque<u32>,
    pub(crate) children: VecDeque<Entry>,
    pub(crate) directories: HashMap<i64, u32>,
    pub(crate) next_handle: u32,
    pub(crate) thumbnails: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FailedBody {
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
    pub(crate) options: DragDropOptions,
    pub(crate) drops_enabled: bool,
    pub(crate) drags_enabled: bool,
    pub(crate) hover: Option<DropOffer>,
    pub(crate) dropped: Option<DropOffer>,
    pub(crate) request: Option<Incoming>,
    pub(crate) received: Received,
    pub(crate) drag: Drag,
    pub(crate) continuation: Option<Message>,
    pub(crate) output_failed: bool,
}

impl DragDrop {
    /// Construct a controller with fixed endpoint identities.
    pub fn new(options: DragDropOptions) -> Self {
        Self {
            options,
            ..Self::default()
        }
    }

    /// Return the unchanged options supplied at construction.
    pub fn options(&self) -> &DragDropOptions {
        &self.options
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

    pub(crate) fn clear_drop(&mut self) {
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

    pub(crate) fn data_target(&self, mime: &str) -> Result<(usize, bool), Error> {
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

    pub(crate) fn discard_continuation(&mut self, kinds: &[&str]) {
        if let Some(message) = &mut self.continuation
            && message.value("t").is_some_and(|kind| kinds.contains(&kind))
        {
            // Drain the remaining frames without reviving an old exchange
            // if the application registers the role again before they arrive.
            message.discarded = true;
            message.payload.clear();
        }
    }

    pub(crate) fn check_output(&self) -> Result<(), Error> {
        if self.output_failed {
            Err(Error::OutputFailed)
        } else {
            Ok(())
        }
    }

    pub(crate) fn write(
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
    /// `max_control_bytes` bounds control payloads across continuation frames.
    /// Incomplete OSC 72 strings return a protocol error.
    ///
    /// Protocol errors require application cleanup. This method preserves
    /// file content and names as bytes and performs no filesystem operations.
    pub fn handle_event(&mut self, event: &uncurses::event::Event) -> Result<Vec<Event>, Error> {
        let (metadata, payload) = match event {
            uncurses::event::Event::DragDrop { options, payload } => (options, payload),
            uncurses::event::Event::UnknownOsc(bytes) => {
                return self.malformed_message(bytes, true);
            }
            uncurses::event::Event::Unknown(bytes) => {
                for prefix in [b"\x1b]".as_slice(), b"\x9d", b"\xc2\x9d"] {
                    if let Some(bytes) = bytes.strip_prefix(prefix) {
                        return self.malformed_message(bytes, false);
                    }
                }
                return Ok(Vec::new());
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
        let result = (|| {
            if message.discarded {
                return Ok(Vec::new());
            }
            if message.value("t") != Some("r")
                && self.options.max_control_bytes.is_some_and(|limit| {
                    payload.len() > limit.saturating_sub(message.payload.len())
                })
            {
                return Err(Error::LimitExceeded("control payload limit exceeded"));
            }
            message.payload.extend_from_slice(payload);
            if message.value("t") == Some("r") {
                self.incoming_data(&mut message, !more, payload.is_empty())
            } else if more {
                Ok(Vec::new())
            } else {
                self.message(&message)
            }
        })();
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

    fn malformed_message(&self, bytes: &[u8], terminated: bool) -> Result<Vec<Event>, Error> {
        let metadata = match bytes.strip_prefix(b"72;") {
            Some(metadata) => metadata,
            None if bytes == b"72" => b"",
            None => return Ok(Vec::new()),
        };
        let end = metadata.iter().position(|byte| *byte == b';');
        let fields = metadata[..end.unwrap_or(metadata.len())].split(|byte| *byte == b':');
        let mut fields = fields.peekable();
        while let Some(field) = fields.next() {
            if let Some(id) = field.strip_prefix(b"i=") {
                // A timed-out final field can still be an unfinished number.
                if (terminated || end.is_some() || fields.peek().is_some())
                    && let Some(id) = std::str::from_utf8(id)
                        .ok()
                        .and_then(|id| id.parse::<u32>().ok())
                    && id != self.options.routing_id.unwrap_or(0)
                {
                    return Ok(Vec::new());
                }
                break;
            }
        }
        Err(Error::Protocol(
            if terminated {
                "malformed OSC 72 message"
            } else {
                "incomplete OSC 72 message"
            }
            .into(),
        ))
    }

    fn check_pending_requests(&self) -> Result<(), Error> {
        if self.options.max_pending_requests.is_some_and(|limit| {
            self.drag
                .requests
                .len()
                .saturating_add(self.drag.file_requests.len())
                >= limit
        }) {
            return Err(Error::LimitExceeded("pending request limit exceeded"));
        }
        Ok(())
    }

    fn message(&mut self, message: &Message) -> Result<Vec<Event>, Error> {
        let mut events = Vec::new();
        let event = match message.value("t") {
            Some("m" | "M") if self.drops_enabled => {
                let actual = message.value("t") == Some("M");
                let leave = !actual && message.number("x")? == -1 && message.number("y")? == -1;
                let offer = if leave {
                    if !message.payload.is_empty() {
                        return Err(Error::Protocol(
                            "leave notification contains MIME types".into(),
                        ));
                    }
                    None
                } else {
                    let mimes = if !actual && message.payload.is_empty() {
                        self.hover
                            .as_ref()
                            .map(|offer| offer.mime_types.clone())
                            .ok_or_else(|| {
                                Error::Protocol("initial hover omitted MIME types".into())
                            })?
                    } else {
                        mime_list(&message.payload)?
                    };
                    Some(DropOffer {
                        position: message.position()?,
                        mime_types: mimes,
                        operations: message.operations()?,
                    })
                };
                if self.dropped.is_some() {
                    self.clear_drop();
                    events.push(Event::DropCanceled);
                }
                let Some(offer) = offer else {
                    self.hover = None;
                    events.push(Event::Leave);
                    return Ok(events);
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
                            self.check_pending_requests()?;
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
                self.check_pending_requests()?;
                self.drag.file_requests.push_back(index);
                Event::EntryRequested {
                    entry: Entry::file(self.drag.scope, 0, index),
                }
            }
            _ => return Ok(Vec::new()),
        };
        events.push(event);
        Ok(events)
    }
}

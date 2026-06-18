//! Terminal queries: send a request, collect the reply.
//!
//! A query pairs a request sequence with a function that recognises its
//! reply, so the two can never drift apart. The leaf query type is
//! [`Single<T>`]; most are predefined constants ([`BACKGROUND_COLOR`],
//! [`PRIMARY_DEVICE_ATTRIBUTES`], …) and a couple are parameterised
//! constructors ([`mode`], [`termcap`]). Build a custom one with
//! [`Single::new`].
//!
//! Run a query with [`EventSource::query`] / [`EventSource::query_blocking`]
//! (threadless) or, over a thread-backed
//! [`EventStream`](super::EventStream), with its `query` / `query_blocking`
//! methods. All four accept the same argument: a single [`Single<T>`], or
//! a *batch* — a tuple or an array of queries. A batch writes every
//! request and flushes once, then collects the replies independently:
//!
//! ```no_run
//! use std::time::Duration;
//! use uncurses::terminal::Terminal;
//! use uncurses::event::{EventSource, query};
//!
//! # fn main() -> std::io::Result<()> {
//! let mut term = Terminal::open()?;
//! term.make_raw()?;
//! let mut out = term.output();
//! let mut source = EventSource::new(term.input())?;
//!
//! // One flush, two replies, each collected on its own.
//! let (bg, da) = source.query_blocking(
//!     &mut out,
//!     (query::BACKGROUND_COLOR, query::PRIMARY_DEVICE_ATTRIBUTES),
//!     Duration::from_millis(100),
//! )?;
//!
//! term.restore()?; // leave raw mode before returning
//! println!("background: {bg:?}, da1: {da:?}");
//! # Ok(())
//! # }
//! ```
//!
//! Replies are *not* hidden from [`read`](EventSource::read): a query
//! observes the event flow non-destructively, so the reply event still
//! arrives through a later read in the order the terminal sent it.
//!
//! The writer is any [`Write`], paired with the source reading the same
//! terminal. If you are already rendering with a
//! [`Screen`](crate::screen::Screen), write the request through it (the
//! bytes are staged and sent on flush in order with any already-staged
//! output); otherwise write straight to a
//! [`Terminal`](crate::terminal::Terminal) handle or stdout.

use std::borrow::Cow;
use std::io::{self, Write};
use std::sync::Arc;
use std::task::Waker as TaskWaker;
use std::time::{Duration, Instant};

use super::source::{Observers, Slot};
use super::{Event, EventSource, Input};
use crate::ansi::mode::{Mode, ModeSetting};
use crate::ansi::{
    KittyKeyboardFlags, background, clipboard, ctrl, graphics, kitty, mode, status, termcap, winop,
    xterm,
};
use crate::color::Color;
use crate::layout::Position;

use super::{ClipboardSelection, ColorScheme, ModifyOtherKeysMode};

mod private {
    /// Sealed: the [`Query`](super::Query) trait is implemented only by
    /// the leaf [`Single`](super::Single) and by tuples and arrays of
    /// queries, so its plumbing methods need never be called by hand.
    pub trait Sealed {}
}

// ---------------------------------------------------------------------------
// The Query trait and the leaf Single<T>
// ---------------------------------------------------------------------------

/// A terminal query that can be issued and have its reply collected.
///
/// Implemented by the leaf [`Single<T>`] and, recursively, by tuples and
/// arrays of queries — so a single value, a heterogeneous tuple, or a
/// homogeneous array can all be passed to
/// [`EventSource::query`]/[`query_blocking`](EventSource::query_blocking).
///
/// The associated types describe the shape of the collected results:
/// [`Replies`](Self::Replies) is the in-flight handle(s) returned by the
/// non-blocking `query`, and [`Resolved`](Self::Resolved) the plain
/// value(s) returned by `query_blocking`. For a [`Single<T>`] these are
/// [`QueryReply<T>`] and `Option<T>`; for a tuple/array they are tuples /
/// arrays of the members' own.
///
/// The trait is sealed; its methods are plumbing and are not meant to be
/// called directly.
pub trait Query: Sized + private::Sealed {
    /// In-flight reply handle(s), returned by the non-blocking query.
    type Replies;
    /// Collected reply value(s), returned by the blocking query.
    type Resolved;

    /// Write the request(s) through `reg` and register the reply
    /// matcher(s), returning the in-flight handle(s). The caller flushes
    /// once afterwards.
    #[doc(hidden)]
    fn issue(self, reg: &mut Registrar<'_>) -> io::Result<Self::Replies>;

    /// Whether every reply in `replies` has resolved (matched or expired).
    #[doc(hidden)]
    fn ready(replies: &Self::Replies) -> bool;

    /// Earliest deadline among the still-pending replies in `replies`.
    #[doc(hidden)]
    fn deadline(replies: &Self::Replies) -> Option<Instant>;

    /// Park `waker` against every still-pending reply in `replies`.
    #[doc(hidden)]
    fn arm(replies: &Self::Replies, waker: &TaskWaker);

    /// Take the collected value(s) from `replies`.
    #[doc(hidden)]
    fn resolve(replies: Self::Replies) -> Self::Resolved;
}

/// A single terminal query: the request sequence to send, and how to
/// recognise the terminal's reply.
///
/// Bundling the request bytes with their reply matcher keeps the two in
/// sync. The matcher is a plain `fn` pointer, so a `Single` is `Copy`,
/// `Send`, and `'static`, and the predefined queries are `const`.
pub struct Single<T> {
    /// Request sequence written to the terminal. Borrowed for the
    /// predefined constants, owned for the parameterised constructors.
    request: Cow<'static, [u8]>,
    /// Maps an event to the query result, or `None` if the event is not
    /// this query's reply.
    reply: fn(&Event) -> Option<T>,
}

impl<T> Single<T> {
    /// Build a query from a static request sequence and a reply matcher.
    pub const fn new(request: &'static [u8], reply: fn(&Event) -> Option<T>) -> Self {
        Self {
            request: Cow::Borrowed(request),
            reply,
        }
    }

    /// Build a query from an owned request sequence and a reply matcher.
    /// Used by the parameterised constructors that encode arguments into
    /// the request bytes.
    pub fn owned(request: Vec<u8>, reply: fn(&Event) -> Option<T>) -> Self {
        Self {
            request: Cow::Owned(request),
            reply,
        }
    }

    /// The request sequence this query writes to the terminal.
    pub fn request(&self) -> &[u8] {
        &self.request
    }

    /// Apply the reply matcher to `event`, returning the result when the
    /// event is this query's reply.
    pub fn matches(&self, event: &Event) -> Option<T> {
        (self.reply)(event)
    }
}

impl<T: 'static> private::Sealed for Single<T> {}

impl<T: 'static> Query for Single<T> {
    type Replies = QueryReply<T>;
    type Resolved = Option<T>;

    fn issue(self, reg: &mut Registrar<'_>) -> io::Result<QueryReply<T>> {
        reg.single(&self.request, self.reply)
    }

    fn ready(replies: &QueryReply<T>) -> bool {
        replies.is_ready()
    }

    fn deadline(replies: &QueryReply<T>) -> Option<Instant> {
        replies.pending_deadline()
    }

    fn arm(replies: &QueryReply<T>, waker: &TaskWaker) {
        replies.arm(waker);
    }

    fn resolve(replies: QueryReply<T>) -> Option<T> {
        let mut replies = replies;
        replies.try_take()
    }
}

// ---------------------------------------------------------------------------
// QueryReply<T> — an in-flight single query
// ---------------------------------------------------------------------------

/// An in-flight terminal query, returned by the non-blocking
/// [`EventSource::query`] (and the thread-backed `EventStream::query`).
///
/// Collect the reply without blocking with [`try_take`](Self::try_take),
/// or — under the `async` feature and a thread-backed
/// [`EventStream`](super::EventStream) — by `.await`ing it (it implements
/// [`Future`](std::future::Future), resolving to `Option<T>`, `None` on
/// timeout). It borrows nothing, so several may be collected together
/// (across threads, or with `join!` / `FuturesUnordered`). Dropping it
/// before it resolves cancels the query.
///
/// On a threadless [`EventSource`], drive the source yourself (e.g. with
/// [`read`](EventSource::read)) and poll with [`try_take`](Self::try_take)
/// / [`is_ready`](Self::is_ready); `.await` only makes progress when a
/// reader thread drives the source.
pub struct QueryReply<T> {
    slot: Arc<Slot>,
    reply: fn(&Event) -> Option<T>,
    /// `None` while pending; `Some(outcome)` once resolved (`outcome`
    /// `Some` = matched value, `None` = expired). Caching the outcome
    /// keeps repeated polls after resolution idempotent.
    value: Option<Option<T>>,
}

impl<T> QueryReply<T> {
    pub(crate) fn new(slot: Arc<Slot>, reply: fn(&Event) -> Option<T>) -> Self {
        Self {
            slot,
            reply,
            value: None,
        }
    }

    /// Collect the reply without blocking, returning `Some(value)` once it
    /// has matched. Returns `None` while still pending *and* once the
    /// query has expired without a reply; use [`is_ready`](Self::is_ready)
    /// to tell those apart.
    pub fn try_take(&mut self) -> Option<T> {
        self.collect();
        match &mut self.value {
            Some(inner) => inner.take(),
            None => None,
        }
    }

    /// Whether the query has resolved (matched or expired).
    pub fn is_ready(&self) -> bool {
        self.value.is_some() || self.slot.is_ready()
    }

    /// Pull the slot's resolution into `value` if it has resolved.
    fn collect(&mut self) {
        if self.value.is_some() {
            return;
        }
        if let Some(outcome) = self.slot.take() {
            self.value = Some(outcome.and_then(|ev| (self.reply)(&ev)));
        }
    }

    /// Park `waker` against the slot while the query is still pending.
    pub(crate) fn arm(&self, waker: &TaskWaker) {
        if self.value.is_none() {
            self.slot.arm(waker);
        }
    }

    /// The query's deadline while it is still pending, else `None`.
    pub(crate) fn pending_deadline(&self) -> Option<Instant> {
        if self.value.is_some() {
            None
        } else {
            self.slot.pending_deadline()
        }
    }
}

#[cfg(feature = "async")]
impl<T: Unpin> std::future::Future for QueryReply<T> {
    type Output = Option<T>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use std::task::Poll;
        let this = self.get_mut();
        this.collect();
        if this.value.is_some() {
            return Poll::Ready(this.value.take().flatten());
        }
        this.slot.arm(cx.waker());
        // Re-check: the reader thread may have resolved the slot between
        // `collect` and arming, and its wake would otherwise be lost.
        this.collect();
        if this.value.is_some() {
            Poll::Ready(this.value.take().flatten())
        } else {
            Poll::Pending
        }
    }
}

// ---------------------------------------------------------------------------
// Registrar — issue glue handed to Query::issue
// ---------------------------------------------------------------------------

/// Issue context handed to [`Query::issue`]: writes request bytes and
/// registers reply matchers against the source's observer registry.
///
/// Opaque; constructed only by the source while running a query.
pub struct Registrar<'a> {
    out: &'a mut dyn Write,
    observers: &'a mut Observers,
    deadline: Instant,
}

impl<'a> Registrar<'a> {
    pub(crate) fn new(
        out: &'a mut dyn Write,
        observers: &'a mut Observers,
        deadline: Instant,
    ) -> Self {
        Self {
            out,
            observers,
            deadline,
        }
    }

    /// Write a single query's request and register its reply matcher.
    pub(crate) fn single<T: 'static>(
        &mut self,
        request: &[u8],
        reply: fn(&Event) -> Option<T>,
    ) -> io::Result<QueryReply<T>> {
        self.out.write_all(request)?;
        let slot = self
            .observers
            .register(Box::new(move |ev| reply(ev).is_some()), self.deadline);
        Ok(QueryReply::new(slot, reply))
    }
}

// ---------------------------------------------------------------------------
// Batch impls: tuples and arrays of queries
// ---------------------------------------------------------------------------

macro_rules! impl_query_tuple {
    ($($q:ident),+) => {
        impl<$($q: Query),+> private::Sealed for ($($q,)+) {}

        impl<$($q: Query),+> Query for ($($q,)+) {
            type Replies = ($($q::Replies,)+);
            type Resolved = ($($q::Resolved,)+);

            #[allow(non_snake_case)]
            fn issue(self, reg: &mut Registrar<'_>) -> io::Result<Self::Replies> {
                let ($($q,)+) = self;
                Ok(($($q.issue(reg)?,)+))
            }

            #[allow(non_snake_case)]
            fn ready(replies: &Self::Replies) -> bool {
                let ($($q,)+) = replies;
                true $(&& $q::ready($q))+
            }

            #[allow(non_snake_case)]
            fn deadline(replies: &Self::Replies) -> Option<Instant> {
                let ($($q,)+) = replies;
                let mut nearest: Option<Instant> = None;
                $(
                    if let Some(d) = $q::deadline($q) {
                        nearest = Some(match nearest {
                            Some(n) => n.min(d),
                            None => d,
                        });
                    }
                )+
                nearest
            }

            #[allow(non_snake_case)]
            fn arm(replies: &Self::Replies, waker: &TaskWaker) {
                let ($($q,)+) = replies;
                $($q::arm($q, waker);)+
            }

            #[allow(non_snake_case)]
            fn resolve(replies: Self::Replies) -> Self::Resolved {
                let ($($q,)+) = replies;
                ($($q::resolve($q),)+)
            }
        }
    };
}

impl_query_tuple!(A);
impl_query_tuple!(A, B);
impl_query_tuple!(A, B, C);
impl_query_tuple!(A, B, C, D);
impl_query_tuple!(A, B, C, D, E);
impl_query_tuple!(A, B, C, D, E, F);
impl_query_tuple!(A, B, C, D, E, F, G);
impl_query_tuple!(A, B, C, D, E, F, G, H);

impl<Q: Query, const N: usize> private::Sealed for [Q; N] {}

impl<Q: Query, const N: usize> Query for [Q; N] {
    type Replies = [Q::Replies; N];
    type Resolved = [Q::Resolved; N];

    fn issue(self, reg: &mut Registrar<'_>) -> io::Result<Self::Replies> {
        let mut replies: Vec<Q::Replies> = Vec::with_capacity(N);
        for q in self {
            replies.push(q.issue(reg)?);
        }
        // `replies` has exactly N elements by construction.
        Ok(replies.try_into().unwrap_or_else(|_| unreachable!()))
    }

    fn ready(replies: &Self::Replies) -> bool {
        replies.iter().all(Q::ready)
    }

    fn deadline(replies: &Self::Replies) -> Option<Instant> {
        replies.iter().filter_map(Q::deadline).min()
    }

    fn arm(replies: &Self::Replies, waker: &TaskWaker) {
        for r in replies {
            Q::arm(r, waker);
        }
    }

    fn resolve(replies: Self::Replies) -> Self::Resolved {
        replies.map(Q::resolve)
    }
}

// ---------------------------------------------------------------------------
// Threadless query methods on EventSource
// ---------------------------------------------------------------------------

impl<I> EventSource<I>
where
    I: Input,
{
    /// Issue `query` (a single query, or a tuple/array batch): write its
    /// request(s) to `out`, flush once, and return the in-flight reply
    /// handle(s) without blocking.
    ///
    /// Drive the source yourself afterwards (e.g. with
    /// [`read`](Self::read)) and collect each handle with
    /// [`QueryReply::try_take`]; non-reply events stay queued, in order,
    /// and a reply event is *also* delivered through a later read. For a
    /// fire-and-collect call that drives the source for you, use
    /// [`query_blocking`](Self::query_blocking).
    pub fn query<Q, W>(
        &mut self,
        out: &mut W,
        query: Q,
        timeout: Duration,
    ) -> io::Result<Q::Replies>
    where
        Q: Query,
        W: Write,
    {
        let deadline = Instant::now() + timeout;
        let replies = {
            let writer: &mut dyn Write = &mut *out;
            let mut reg = Registrar::new(writer, self.observers_mut(), deadline);
            query.issue(&mut reg)?
        };
        out.flush()?;
        Ok(replies)
    }

    /// Issue `query` and block up to `timeout`, driving the source inline
    /// until every reply has arrived (or its deadline elapsed), then
    /// return the collected value(s).
    ///
    /// A single query yields `Option<T>` (`None` on timeout); a batch
    /// yields a tuple/array of each member's own, collected independently
    /// so one reply timing out does not lose the others. Non-reply events
    /// stay queued, in order, for a later [`read`](Self::read), and each
    /// reply event is also delivered through a later read.
    #[cfg(any(unix, windows))]
    pub fn query_blocking<Q, W>(
        &mut self,
        out: &mut W,
        query: Q,
        timeout: Duration,
    ) -> io::Result<Q::Resolved>
    where
        Q: Query,
        W: Write,
    {
        let replies = self.query(out, query, timeout)?;
        loop {
            if Q::ready(&replies) {
                break;
            }
            let now = Instant::now();
            let remaining = match Q::deadline(&replies) {
                Some(d) if d > now => d - now,
                _ => break,
            };
            match self.pump(Some(remaining)) {
                Ok(()) => {}
                // A paired waker fired; keep waiting until the deadline.
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(Q::resolve(replies))
    }
}

// ---------------------------------------------------------------------------
// Predefined queries
// ---------------------------------------------------------------------------

/// Primary device attributes (`CSI c`).
pub const PRIMARY_DEVICE_ATTRIBUTES: Single<Vec<Option<u32>>> =
    Single::new(ctrl::REQUEST_PRIMARY_DA, |ev| match ev {
        Event::PrimaryDeviceAttributes(v) => Some(v.clone()),
        _ => None,
    });

/// Secondary device attributes (`CSI > c`).
pub const SECONDARY_DEVICE_ATTRIBUTES: Single<Vec<Option<u32>>> =
    Single::new(ctrl::REQUEST_SECONDARY_DA, |ev| match ev {
        Event::SecondaryDeviceAttributes(v) => Some(v.clone()),
        _ => None,
    });

/// Tertiary device attributes (`CSI = c`).
pub const TERTIARY_DEVICE_ATTRIBUTES: Single<String> =
    Single::new(ctrl::REQUEST_TERTIARY_DA, |ev| match ev {
        Event::TertiaryDeviceAttributes(s) => Some(s.clone()),
        _ => None,
    });

/// Terminal name and version (XTVERSION, `CSI > q`).
pub const TERMINAL_VERSION: Single<String> = Single::new(ctrl::REQUEST_XTVERSION, |ev| match ev {
    Event::TerminalVersion(s) => Some(s.clone()),
    _ => None,
});

/// Active kitty keyboard protocol flags (`CSI ? u`).
pub const KITTY_KEYBOARD_FLAGS: Single<KittyKeyboardFlags> =
    Single::new(kitty::REQUEST_KITTY_KEYBOARD, |ev| match ev {
        Event::KittyKeyboardEnhancements(f) => Some(*f),
        _ => None,
    });

/// Current `modifyOtherKeys` mode (`CSI ? 4 m`).
pub const MODIFY_OTHER_KEYS: Single<ModifyOtherKeysMode> =
    Single::new(xterm::QUERY_MODIFY_OTHER_KEYS, |ev| match ev {
        Event::ModifyOtherKeys(m) => Some(*m),
        _ => None,
    });

/// Default foreground color (`OSC 10 ; ?`).
pub const FOREGROUND_COLOR: Single<Color> =
    Single::new(background::REQUEST_FOREGROUND_COLOR, |ev| match ev {
        Event::ForegroundColor(c) => Some(*c),
        _ => None,
    });

/// Default background color (`OSC 11 ; ?`).
pub const BACKGROUND_COLOR: Single<Color> =
    Single::new(background::REQUEST_BACKGROUND_COLOR, |ev| match ev {
        Event::BackgroundColor(c) => Some(*c),
        _ => None,
    });

/// Cursor color (`OSC 12 ; ?`).
pub const CURSOR_COLOR: Single<Color> =
    Single::new(background::REQUEST_CURSOR_COLOR, |ev| match ev {
        Event::CursorColor(c) => Some(*c),
        _ => None,
    });

/// Character cell pixel size (`CSI 16 t`), as `(width, height)`.
pub const CELL_PIXEL_SIZE: Single<(u16, u16)> =
    Single::new(winop::REQUEST_CELL_PIXEL_SIZE, |ev| match ev {
        Event::CellPixelSize { width, height } => Some((*width, *height)),
        _ => None,
    });

/// Window pixel size (`CSI 14 t`), as `(width, height)`.
pub const WINDOW_PIXEL_SIZE: Single<(u16, u16)> =
    Single::new(winop::REQUEST_WINDOW_PIXEL_SIZE, |ev| match ev {
        Event::WindowPixelSize { width, height } => Some((*width, *height)),
        _ => None,
    });

/// Cursor position report (CPR, `CSI 6 n`).
pub const CURSOR_POSITION: Single<Position> =
    Single::new(status::REQUEST_CURSOR_POSITION, |ev| match ev {
        Event::CursorPosition(p) => Some(*p),
        _ => None,
    });

/// Current terminal color scheme (DEC 2031, `CSI ? 996 n`).
pub const COLOR_SCHEME: Single<ColorScheme> =
    Single::new(status::REQUEST_LIGHT_DARK_REPORT, |ev| match ev {
        Event::DarkColorScheme => Some(ColorScheme::Dark),
        Event::LightColorScheme => Some(ColorScheme::Light),
        _ => None,
    });

/// Whether in-band resize notifications (DEC 2048) are enabled (DECRQM,
/// `CSI ? 2048 $p`). The reply is the reported [`ModeSetting`] for mode
/// 2048; a [`ModeSetting::Set`] terminal emits [`Event::Resize`] in-band
/// on every surface size change.
///
/// [`Event::Resize`]: crate::event::Event::Resize
pub const IN_BAND_RESIZE: Single<ModeSetting> = Single::new(b"\x1b[?2048$p", |ev| match ev {
    Event::ModeReport { mode, setting } if *mode == Mode::IN_BAND_RESIZE => Some(*setting),
    _ => None,
});

/// Query the current setting of a terminal mode (DECRQM). Handles both
/// ANSI modes (`CSI mode $p`) and DEC private modes (`CSI ? mode $p`).
///
/// The reply is the reported [`ModeSetting`] of the first `ModeReport`
/// that arrives.
pub fn mode(m: Mode) -> Single<ModeSetting> {
    let mut request = Vec::new();
    mode::write_request_mode(&mut request, m).expect("encoding a mode request cannot fail");
    Single::owned(request, |ev| match ev {
        Event::ModeReport { setting, .. } => Some(*setting),
        _ => None,
    })
}

/// Query termcap entries by short name (`DCS + q ... ST`). The reply is
/// the decoded `Termcap` payload string.
pub fn termcap(names: &[&str]) -> Single<String> {
    let mut request = Vec::new();
    termcap::write_xtgettcap(&mut request, names).expect("encoding a termcap request cannot fail");
    Single::owned(request, |ev| match ev {
        Event::Termcap(s) => Some(s.clone()),
        _ => None,
    })
}

/// Reply to a [`kitty_graphics`] query: the response key/value options
/// and the payload (which typically carries a status such as `OK`).
pub type KittyGraphicsReply = (Vec<(String, String)>, Vec<u8>);

/// Query the kitty graphics protocol (`APC G a=q,<options> ST`), sent
/// with an empty payload. The query action `a=q` is enforced: it is
/// added when `options` omits an `a=` directive, and any `a=` directive
/// that is not `a=q` is replaced with `a=q`. So `options` carries the
/// image descriptors — e.g. `["t=d", "i=1", "s=1", "v=1"]` to probe
/// support with a 1×1 in-memory image.
///
/// The reply is the response `(options, payload)`; the payload typically
/// carries the status (`OK` or an error). Terminals that don't speak the
/// protocol stay silent and the query times out.
pub fn kitty_graphics(options: &[&str]) -> Single<KittyGraphicsReply> {
    // Enforce the query action `a=q`: replace any caller-supplied `a=`
    // directive that isn't `q`, and add one if absent, so this can never
    // be a transmission by mistake.
    let mut directives: Vec<&str> = Vec::with_capacity(options.len() + 1);
    let mut has_action = false;
    for &opt in options {
        if opt.starts_with("a=") {
            if !has_action {
                directives.push("a=q");
                has_action = true;
            }
        } else {
            directives.push(opt);
        }
    }
    if !has_action {
        directives.push("a=q");
    }
    let mut request = Vec::new();
    graphics::write_kitty_graphics(&mut request, &directives, &[])
        .expect("encoding a kitty graphics request cannot fail");
    Single::owned(request, |ev| match ev {
        Event::KittyGraphics { options, payload } => Some((options.clone(), payload.clone())),
        _ => None,
    })
}

/// Read a clipboard selection (`OSC 52 ; Pc ; ?`). The reply is the
/// decoded clipboard text. Most terminals disable clipboard reads by
/// default (a privacy measure) and stay silent, so pair this with the
/// [`PRIMARY_DEVICE_ATTRIBUTES`] sentinel to tell "denied" from "slow".
///
/// The reply matcher only accepts a report for the *same* selection, so
/// concurrent reads of different selections never cross-match.
pub fn read_clipboard(selection: ClipboardSelection) -> Single<String> {
    let pc = match selection {
        ClipboardSelection::System => clipboard::SYSTEM_CLIPBOARD,
        ClipboardSelection::Primary => clipboard::PRIMARY_CLIPBOARD,
        ClipboardSelection::Other(c) => c as u8,
    };
    // Pick a non-capturing matcher per selection so it stays a `fn`
    // pointer. `Other` matches any non-standard selection char.
    let reply: fn(&Event) -> Option<String> = match selection {
        ClipboardSelection::System => |ev| clipboard_reply(ev, ClipboardSelection::System),
        ClipboardSelection::Primary => |ev| clipboard_reply(ev, ClipboardSelection::Primary),
        ClipboardSelection::Other(_) => |ev| match ev {
            Event::Clipboard {
                selection: ClipboardSelection::Other(_),
                content,
            } => Some(content.clone()),
            _ => None,
        },
    };
    let mut request = Vec::new();
    clipboard::write_request_clipboard(&mut request, pc)
        .expect("encoding a clipboard request cannot fail");
    Single::owned(request, reply)
}

fn clipboard_reply(ev: &Event, want: ClipboardSelection) -> Option<String> {
    match ev {
        Event::Clipboard { selection, content } if *selection == want => Some(content.clone()),
        _ => None,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::os::unix::io::AsRawFd;

    use super::*;
    use crate::event::{Key, KeyCode};

    fn make_pipe() -> (File, File) {
        let mut fds = [0i32; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "pipe(2) failed");
        // SAFETY: pipe(2) just produced two fresh, owned fds.
        let rx = unsafe { File::from_raw_fd(fds[0]) };
        let tx = unsafe { File::from_raw_fd(fds[1]) };
        (rx, tx)
    }

    fn write_bytes(f: &File, bytes: &[u8]) {
        let n = unsafe { libc::write(f.as_raw_fd(), bytes.as_ptr() as *const _, bytes.len()) };
        assert_eq!(n, bytes.len() as isize);
    }

    /// A query resolving on `KeyPress('b')`, for deterministic pipe-driven
    /// tests without a real terminal reply.
    const KEY_B: Single<char> = Single::new(b"REQ", reply_key_b);
    fn reply_key_b(ev: &Event) -> Option<char> {
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('b'),
                ..
            }) => Some('b'),
            _ => None,
        }
    }

    /// Companion query resolving on `KeyPress('c')`.
    const KEY_C: Single<char> = Single::new(b"REQ2", reply_key_c);
    fn reply_key_c(ev: &Event) -> Option<char> {
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                ..
            }) => Some('c'),
            _ => None,
        }
    }

    #[test]
    fn query_blocking_resolves_reply_keeping_input_visible() {
        let (rx, tx) = make_pipe();
        let mut source = EventSource::new(rx).unwrap();
        // A user keypress 'a' arrives before the awaited reply 'b'.
        write_bytes(&tx, b"ab");
        let mut out: Vec<u8> = Vec::new();
        let got = source
            .query_blocking(&mut out, KEY_B, Duration::from_secs(1))
            .unwrap();
        assert_eq!(got, Some('b'));
        // The request was written and flushed.
        assert_eq!(out, b"REQ");
        // Non-reply input stays queued; the reply 'b' is also still
        // visible (non-destructive), so both arrive through read in order.
        let a = source.read().unwrap();
        assert!(matches!(a, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
        let b = source.read().unwrap();
        assert!(matches!(b, Event::KeyPress(k) if k.code == KeyCode::Char('b')));
    }

    #[test]
    fn query_blocking_times_out_without_reply() {
        let (rx, tx) = make_pipe();
        let mut source = EventSource::new(rx).unwrap();
        write_bytes(&tx, b"a");
        let mut out: Vec<u8> = Vec::new();
        let got = source
            .query_blocking(&mut out, KEY_B, Duration::from_millis(30))
            .unwrap();
        assert_eq!(got, None);
        // The user input is untouched by the failed query.
        let ev = source.read().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
    }

    #[test]
    fn query_blocking_batch_one_flush_independent_replies() {
        let (rx, tx) = make_pipe();
        let mut source = EventSource::new(rx).unwrap();
        // Both replies arrive interleaved with user input 'a'.
        write_bytes(&tx, b"cab");
        let mut out: Vec<u8> = Vec::new();
        let (b, c) = source
            .query_blocking(&mut out, (KEY_B, KEY_C), Duration::from_secs(1))
            .unwrap();
        assert_eq!(b, Some('b'));
        assert_eq!(c, Some('c'));
        // One flush wrote both requests, in order.
        assert_eq!(out, b"REQREQ2");
    }

    #[test]
    fn query_blocking_array_batch() {
        let (rx, tx) = make_pipe();
        let mut source = EventSource::new(rx).unwrap();
        write_bytes(&tx, b"bc");
        let mut out: Vec<u8> = Vec::new();
        let [b, c] = source
            .query_blocking(&mut out, [KEY_B, KEY_C], Duration::from_secs(1))
            .unwrap();
        assert_eq!(b, Some('b'));
        assert_eq!(c, Some('c'));
        assert_eq!(out, b"REQREQ2");
    }

    #[test]
    fn query_returns_handles_driven_by_read() {
        let (rx, tx) = make_pipe();
        let mut source = EventSource::new(rx).unwrap();
        let mut out: Vec<u8> = Vec::new();
        let mut handle = source
            .query(&mut out, KEY_B, Duration::from_secs(1))
            .unwrap();
        assert_eq!(out, b"REQ");
        assert!(handle.try_take().is_none());
        // Drive the source; the reply fills the handle's slot.
        write_bytes(&tx, b"b");
        let ev = source.read().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('b')));
        assert_eq!(handle.try_take(), Some('b'));
    }

    #[test]
    fn in_band_resize_query_matches_mode_2048_report() {
        // The request is the DECRQM probe for DEC private mode 2048.
        assert_eq!(IN_BAND_RESIZE.request(), b"\x1b[?2048$p");
        // It resolves on a ModeReport for mode 2048, ignoring others.
        let ev = Event::ModeReport {
            mode: Mode::IN_BAND_RESIZE,
            setting: ModeSetting::Set,
        };
        assert_eq!(IN_BAND_RESIZE.matches(&ev), Some(ModeSetting::Set));
        let other = Event::ModeReport {
            mode: Mode::Dec(2031),
            setting: ModeSetting::Set,
        };
        assert_eq!(IN_BAND_RESIZE.matches(&other), None);
    }

    #[test]
    fn parameterised_queries_encode_their_arguments() {
        // `mode` and `termcap` build their request bytes from arguments.
        let m = mode(Mode::Dec(2026));
        assert_eq!(m.request(), b"\x1b[?2026$p");
        let tc = termcap(&["Co", "TN"]);
        assert!(tc.request().starts_with(b"\x1bP+q"));
    }

    #[test]
    fn kitty_graphics_enforces_query_action() {
        // No `a=` directive: `a=q` is added.
        let q = kitty_graphics(&["i=1", "s=1", "v=1"]);
        assert_eq!(q.request(), b"\x1b_Gi=1,s=1,v=1,a=q\x1b\\");
        // A conflicting `a=t`: replaced with `a=q`, position preserved.
        let q = kitty_graphics(&["a=t", "i=1"]);
        assert_eq!(q.request(), b"\x1b_Ga=q,i=1\x1b\\");
        // Already `a=q`: left as a single `a=q`.
        let q = kitty_graphics(&["a=q", "i=1"]);
        assert_eq!(q.request(), b"\x1b_Ga=q,i=1\x1b\\");
    }

    #[test]
    fn read_clipboard_matches_only_its_selection() {
        let system = Event::Clipboard {
            selection: ClipboardSelection::System,
            content: "sys".to_string(),
        };
        let primary = Event::Clipboard {
            selection: ClipboardSelection::Primary,
            content: "pri".to_string(),
        };

        let q = read_clipboard(ClipboardSelection::System);
        assert_eq!(q.request(), b"\x1b]52;c;?\x07");
        // Matches its own selection, ignores another selection's reply so
        // concurrent reads never cross-match.
        assert_eq!(q.matches(&system), Some("sys".to_string()));
        assert_eq!(q.matches(&primary), None);

        let q = read_clipboard(ClipboardSelection::Primary);
        assert_eq!(q.request(), b"\x1b]52;p;?\x07");
        assert_eq!(q.matches(&primary), Some("pri".to_string()));
        assert_eq!(q.matches(&system), None);
    }
}

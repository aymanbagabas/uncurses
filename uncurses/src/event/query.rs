//! Terminal queries: send a request, await the reply.
//!
//! A [`Query`] pairs a request sequence with a function that recognises
//! its reply, so the two can never drift apart. Run one with
//! [`EventSource::query`] (synchronous) or, behind the `async` feature,
//! [`EventStream::query`](crate::event::EventStream::query). The reply is
//! plucked out of the event stream with [`EventSource::read_matching`], so
//! any user input that arrives meanwhile stays queued, in order, for a
//! later [`EventSource::read`] — querying never drops input.
//!
//! The writer is any [`Write`]: a [`Terminal`](crate::Terminal) handle, a
//! [`Screen`](crate::screen::Screen) (writes are staged and flushed), or
//! a bare stdout. Pair it with the [`EventSource`] reading the same
//! terminal.
//!
//! ```no_run
//! use std::time::Duration;
//! use uncurses::Terminal;
//! use uncurses::event::{EventSource, query};
//!
//! # fn main() -> std::io::Result<()> {
//! let mut term = Terminal::open()?;
//! let _prev = term.make_raw()?;
//! let mut out = term.output();
//! let mut source = EventSource::new(term.input())?;
//! let bg = source.query(&mut out, &query::BACKGROUND_COLOR, Duration::from_millis(100))?;
//! println!("background: {bg:?}");
//! # Ok(())
//! # }
//! ```

use std::io::{self, Write};
use std::time::Duration;

use super::{Event, EventSource, Input};
use crate::ansi::{background, ctrl};
use crate::color::Color;

/// A terminal query: the request sequence to send, and how to recognise
/// the terminal's reply.
///
/// Bundling the request bytes with their reply matcher keeps the two in
/// sync and lets a single query value drive both the synchronous
/// [`EventSource::query`] and the asynchronous
/// [`EventStream::query`](crate::event::EventStream::query).
///
/// Predefined queries are provided as constants ([`BACKGROUND_COLOR`],
/// [`PRIMARY_DEVICE_ATTRIBUTES`]). Construct a custom one with
/// [`Query::new`].
pub struct Query<T> {
    /// Request sequence written to the terminal.
    request: &'static [u8],
    /// Maps an event to the query result, or `None` if the event is not
    /// this query's reply. A plain `fn` pointer so the value is `Copy`,
    /// `Send`, and `'static` — usable from the async reader thread
    /// without boxing the matcher itself.
    reply: fn(&Event) -> Option<T>,
}

impl<T> Query<T> {
    /// Build a query from a request sequence and a reply matcher.
    pub const fn new(request: &'static [u8], reply: fn(&Event) -> Option<T>) -> Self {
        Self { request, reply }
    }

    /// The request sequence this query writes to the terminal.
    pub fn request(&self) -> &'static [u8] {
        self.request
    }

    /// Apply the reply matcher to `event`, returning the result when the
    /// event is this query's reply.
    pub fn matches(&self, event: &Event) -> Option<T> {
        (self.reply)(event)
    }

    /// The reply matcher as a plain function pointer (`Copy`, `Send`,
    /// `'static`), for dispatching a query to the async reader thread.
    #[cfg(feature = "async")]
    pub(crate) fn reply_fn(&self) -> fn(&Event) -> Option<T> {
        self.reply
    }
}

/// Query the terminal's default background color (`OSC 11`).
pub const BACKGROUND_COLOR: Query<Color> =
    Query::new(background::REQUEST_BACKGROUND_COLOR, reply_background_color);

/// Query the terminal's primary device attributes (`CSI c`).
pub const PRIMARY_DEVICE_ATTRIBUTES: Query<Vec<Option<u32>>> =
    Query::new(ctrl::REQUEST_PRIMARY_DA, reply_primary_da);

fn reply_background_color(event: &Event) -> Option<Color> {
    match event {
        Event::BackgroundColor(c) => Some(*c),
        _ => None,
    }
}

fn reply_primary_da(event: &Event) -> Option<Vec<Option<u32>>> {
    match event {
        Event::PrimaryDeviceAttributes(v) => Some(v.clone()),
        _ => None,
    }
}

impl<I> EventSource<I>
where
    I: Input,
{
    /// Run `query`: write its request to `out`, then wait up to `timeout`
    /// for the reply, returning its mapped value.
    ///
    /// Events that are not the reply are left queued, in order, for a
    /// later [`read`](EventSource::read). Returns `Ok(None)` if the
    /// terminal does not reply within `timeout` (a late reply is ignored).
    pub fn query<W, T>(
        &mut self,
        out: &mut W,
        query: &Query<T>,
        timeout: Duration,
    ) -> io::Result<Option<T>>
    where
        W: Write,
    {
        out.write_all(query.request)?;
        out.flush()?;
        match self.read_matching(|ev| query.matches(ev).is_some(), Some(timeout))? {
            Some(ev) => Ok(query.matches(&ev)),
            None => Ok(None),
        }
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

    /// A query that resolves on a `KeyPress('b')`, for deterministic
    /// pipe-driven tests without a real terminal reply.
    const KEY_B: Query<()> = Query::new(b"REQ", reply_key_b);
    fn reply_key_b(ev: &Event) -> Option<()> {
        matches!(
            ev,
            Event::KeyPress(Key {
                code: KeyCode::Char('b'),
                ..
            })
        )
        .then_some(())
    }

    #[test]
    fn query_writes_then_plucks_reply_leaving_others() {
        let (rx, tx) = make_pipe();
        let mut source = EventSource::new(rx).unwrap();
        // A user keypress 'a' arrives before the awaited reply 'b'.
        write_bytes(&tx, b"ab");
        let mut out: Vec<u8> = Vec::new();
        let got = source
            .query(&mut out, &KEY_B, Duration::from_secs(1))
            .unwrap();
        assert!(got.is_some(), "reply should be found");
        // The request was written to the output and flushed.
        assert_eq!(out, b"REQ");
        // The non-matching 'a' stays queued, in order, for a normal read.
        let ev = source.read().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
    }

    #[test]
    fn query_times_out_without_reply() {
        let (rx, tx) = make_pipe();
        let mut source = EventSource::new(rx).unwrap();
        write_bytes(&tx, b"a");
        let mut out: Vec<u8> = Vec::new();
        let got = source
            .query(&mut out, &KEY_B, Duration::from_millis(30))
            .unwrap();
        assert!(got.is_none());
        // The user input is untouched by the failed query.
        let ev = source.read().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
    }
}

//! Terminal query helpers: send a request, await the reply.
//!
//! A query writes a request sequence to an output, then reads the
//! terminal's reply off an input [`EventSource`]. The reply is plucked out of
//! the source's event queue with [`EventSource::read_matching`], so any user
//! input that arrives meanwhile stays queued, in order, for a later
//! [`EventSource::read`] — querying never drops input.
//!
//! The writer is any [`Write`]: a [`Terminal`](crate::Terminal) handle, a
//! [`Screen`](crate::screen::Screen) (writes are staged and flushed), or
//! a bare stdout. Pair it with the [`EventSource`] reading the same terminal.
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
//! let bg = query::background_color(&mut out, &mut source, Duration::from_millis(100))?;
//! println!("background: {bg:?}");
//! # Ok(())
//! # }
//! ```

use std::io::{self, Write};
use std::time::Duration;

use super::{Event, EventSource, Input};
use crate::ansi::{background, ctrl};
use crate::color::Color;

/// Write `request`, then wait up to `timeout` for an event that `matcher`
/// accepts, returning its mapped value.
///
/// Events that do not match are left queued in `source`, in order, for a
/// later [`EventSource::read`]. Returns `Ok(None)` if the terminal does not
/// reply within `timeout` (a late reply is ignored).
///
/// This is the building block for the typed helpers in this module and
/// for queries they do not cover.
pub fn request<W, I, T>(
    out: &mut W,
    source: &mut EventSource<I>,
    request: &[u8],
    matcher: impl Fn(&Event) -> Option<T>,
    timeout: Duration,
) -> io::Result<Option<T>>
where
    W: Write,
    I: Input,
{
    out.write_all(request)?;
    out.flush()?;
    match source.read_matching(|ev| matcher(ev).is_some(), Some(timeout))? {
        Some(ev) => Ok(matcher(&ev)),
        None => Ok(None),
    }
}

/// Query the terminal's default background color (`OSC 11`).
///
/// Returns `None` if the terminal does not reply within `timeout`.
pub fn background_color<W, I>(
    out: &mut W,
    source: &mut EventSource<I>,
    timeout: Duration,
) -> io::Result<Option<Color>>
where
    W: Write,
    I: Input,
{
    request(
        out,
        source,
        background::REQUEST_BACKGROUND_COLOR,
        |ev| match ev {
            Event::BackgroundColor(c) => Some(*c),
            _ => None,
        },
        timeout,
    )
}

/// Query the terminal's primary device attributes (`CSI c`).
///
/// Returns `None` if the terminal does not reply within `timeout`.
pub fn primary_device_attributes<W, I>(
    out: &mut W,
    source: &mut EventSource<I>,
    timeout: Duration,
) -> io::Result<Option<Vec<Option<u32>>>>
where
    W: Write,
    I: Input,
{
    request(
        out,
        source,
        ctrl::REQUEST_PRIMARY_DA,
        |ev| match ev {
            Event::PrimaryDeviceAttributes(v) => Some(v.clone()),
            _ => None,
        },
        timeout,
    )
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::os::unix::io::AsRawFd;

    use super::*;
    use crate::event::KeyCode;

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

    #[test]
    fn request_writes_then_plucks_matching_leaving_others() {
        let (rx, tx) = make_pipe();
        let mut source = EventSource::new(rx).unwrap();
        // A user keypress 'a' arrives before the awaited reply 'b'.
        write_bytes(&tx, b"ab");
        let mut out: Vec<u8> = Vec::new();
        let got = request(
            &mut out,
            &mut source,
            b"REQ",
            |ev| matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('b')).then_some(()),
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(got.is_some(), "reply should be found");
        // The request was written to the output and flushed.
        assert_eq!(out, b"REQ");
        // The non-matching 'a' stays queued, in order, for a normal read.
        let ev = source.read().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
    }

    #[test]
    fn request_times_out_without_reply() {
        let (rx, tx) = make_pipe();
        let mut source = EventSource::new(rx).unwrap();
        write_bytes(&tx, b"a");
        let mut out: Vec<u8> = Vec::new();
        let got: Option<()> = request(
            &mut out,
            &mut source,
            b"REQ",
            |ev| matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('z')).then_some(()),
            Duration::from_millis(30),
        )
        .unwrap();
        assert!(got.is_none());
        // The user input is untouched by the failed query.
        let ev = source.read().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
    }
}

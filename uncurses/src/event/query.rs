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
//! Most queries are predefined constants ([`BACKGROUND_COLOR`],
//! [`PRIMARY_DEVICE_ATTRIBUTES`], …); the two parameterised ones
//! ([`mode`], [`termcap`]) are constructor functions. Build a custom
//! query with [`Query::new`].
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

use std::borrow::Cow;
use std::io::{self, Write};
use std::time::Duration;

use super::{Event, EventSource, Input};
use crate::Position;
use crate::ansi::mode::{Mode, ModeSetting};
use crate::ansi::{
    KittyKeyboardFlags, background, ctrl, graphics, kitty, mode, status, termcap, winop, xterm,
};
use crate::color::Color;

use super::{ColorScheme, ModifyOtherKeysMode};

/// A terminal query: the request sequence to send, and how to recognise
/// the terminal's reply.
///
/// Bundling the request bytes with their reply matcher keeps the two in
/// sync and lets a single query value drive both the synchronous
/// [`EventSource::query`] and the asynchronous
/// [`EventStream::query`](crate::event::EventStream::query).
pub struct Query<T> {
    /// Request sequence written to the terminal. Borrowed for the
    /// predefined constants, owned for the parameterised constructors.
    request: Cow<'static, [u8]>,
    /// Maps an event to the query result, or `None` if the event is not
    /// this query's reply. A plain `fn` pointer so the value is `Copy`,
    /// `Send`, and `'static` — usable from the async reader thread
    /// without boxing the matcher itself.
    reply: fn(&Event) -> Option<T>,
}

impl<T> Query<T> {
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

    /// The reply matcher as a plain function pointer (`Copy`, `Send`,
    /// `'static`), for dispatching a query to the async reader thread.
    #[cfg(feature = "async")]
    pub(crate) fn reply_fn(&self) -> fn(&Event) -> Option<T> {
        self.reply
    }
}

/// Primary device attributes (`CSI c`).
pub const PRIMARY_DEVICE_ATTRIBUTES: Query<Vec<Option<u32>>> =
    Query::new(ctrl::REQUEST_PRIMARY_DA, |ev| match ev {
        Event::PrimaryDeviceAttributes(v) => Some(v.clone()),
        _ => None,
    });

/// Secondary device attributes (`CSI > c`).
pub const SECONDARY_DEVICE_ATTRIBUTES: Query<Vec<Option<u32>>> =
    Query::new(ctrl::REQUEST_SECONDARY_DA, |ev| match ev {
        Event::SecondaryDeviceAttributes(v) => Some(v.clone()),
        _ => None,
    });

/// Tertiary device attributes (`CSI = c`).
pub const TERTIARY_DEVICE_ATTRIBUTES: Query<String> =
    Query::new(ctrl::REQUEST_TERTIARY_DA, |ev| match ev {
        Event::TertiaryDeviceAttributes(s) => Some(s.clone()),
        _ => None,
    });

/// Terminal name and version (XTVERSION, `CSI > q`).
pub const TERMINAL_VERSION: Query<String> = Query::new(ctrl::REQUEST_XTVERSION, |ev| match ev {
    Event::TerminalVersion(s) => Some(s.clone()),
    _ => None,
});

/// Active kitty keyboard protocol flags (`CSI ? u`).
pub const KITTY_KEYBOARD_FLAGS: Query<KittyKeyboardFlags> =
    Query::new(kitty::REQUEST_KITTY_KEYBOARD, |ev| match ev {
        Event::KittyKeyboardEnhancements(f) => Some(*f),
        _ => None,
    });

/// Current `modifyOtherKeys` mode (`CSI ? 4 m`).
pub const MODIFY_OTHER_KEYS: Query<ModifyOtherKeysMode> =
    Query::new(xterm::QUERY_MODIFY_OTHER_KEYS, |ev| match ev {
        Event::ModifyOtherKeys(m) => Some(*m),
        _ => None,
    });

/// Default foreground color (`OSC 10 ; ?`).
pub const FOREGROUND_COLOR: Query<Color> =
    Query::new(background::REQUEST_FOREGROUND_COLOR, |ev| match ev {
        Event::ForegroundColor(c) => Some(*c),
        _ => None,
    });

/// Default background color (`OSC 11 ; ?`).
pub const BACKGROUND_COLOR: Query<Color> =
    Query::new(background::REQUEST_BACKGROUND_COLOR, |ev| match ev {
        Event::BackgroundColor(c) => Some(*c),
        _ => None,
    });

/// Cursor color (`OSC 12 ; ?`).
pub const CURSOR_COLOR: Query<Color> =
    Query::new(background::REQUEST_CURSOR_COLOR, |ev| match ev {
        Event::CursorColor(c) => Some(*c),
        _ => None,
    });

/// Character cell pixel size (`CSI 16 t`), as `(width, height)`.
pub const CELL_PIXEL_SIZE: Query<(u16, u16)> =
    Query::new(winop::REQUEST_CELL_PIXEL_SIZE, |ev| match ev {
        Event::CellPixelSize { width, height } => Some((*width, *height)),
        _ => None,
    });

/// Window pixel size (`CSI 14 t`), as `(width, height)`.
pub const WINDOW_PIXEL_SIZE: Query<(u16, u16)> =
    Query::new(winop::REQUEST_WINDOW_PIXEL_SIZE, |ev| match ev {
        Event::WindowPixelSize { width, height } => Some((*width, *height)),
        _ => None,
    });

/// Cursor position report (CPR, `CSI 6 n`).
pub const CURSOR_POSITION: Query<Position> =
    Query::new(status::REQUEST_CURSOR_POSITION, |ev| match ev {
        Event::CursorPosition(p) => Some(*p),
        _ => None,
    });

/// Current terminal color scheme (DEC 2031, `CSI ? 996 n`).
pub const COLOR_SCHEME: Query<ColorScheme> =
    Query::new(status::REQUEST_LIGHT_DARK_REPORT, |ev| match ev {
        Event::DarkColorScheme => Some(ColorScheme::Dark),
        Event::LightColorScheme => Some(ColorScheme::Light),
        _ => None,
    });

/// Query the current setting of a terminal mode (DECRQM). Handles both
/// ANSI modes (`CSI mode $p`) and DEC private modes (`CSI ? mode $p`).
///
/// The reply is the reported [`ModeSetting`] of the first `ModeReport`
/// that arrives.
pub fn mode(m: Mode) -> Query<ModeSetting> {
    let mut request = Vec::new();
    mode::write_request_mode(&mut request, m).expect("encoding a mode request cannot fail");
    Query::owned(request, |ev| match ev {
        Event::ModeReport { setting, .. } => Some(*setting),
        _ => None,
    })
}

/// Query termcap entries by short name (`DCS + q ... ST`). The reply is
/// the decoded `Termcap` payload string.
pub fn termcap(names: &[&str]) -> Query<String> {
    let mut request = Vec::new();
    termcap::write_xtgettcap(&mut request, names).expect("encoding a termcap request cannot fail");
    Query::owned(request, |ev| match ev {
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
pub fn kitty_graphics(options: &[&str]) -> Query<KittyGraphicsReply> {
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
    Query::owned(request, |ev| match ev {
        Event::KittyGraphics { options, payload } => Some((options.clone(), payload.clone())),
        _ => None,
    })
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
        out.write_all(query.request())?;
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
    const KEY_B: Query<()> = Query::new(b"REQ", |ev| {
        matches!(
            ev,
            Event::KeyPress(Key {
                code: KeyCode::Char('b'),
                ..
            })
        )
        .then_some(())
    });

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
}

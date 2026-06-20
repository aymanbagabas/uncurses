//! Terminal events and event-stream decoding.
//!
//! This module owns the core [`Event`] enum together with the
//! [`Decoder`] that parses raw terminal bytes into events, the
//! platform-specific [`EventSource`] that drives the decoder from a
//! tty, and the key/mouse types that events carry.

pub mod decode;
#[cfg(test)]
mod decode_safety_tests;
mod key;
mod mouse;
mod pending;
pub mod poll;
mod sigwinch;
pub mod source;
#[cfg(unix)]
pub mod source_unix;
#[cfg(windows)]
pub mod source_windows;
#[cfg(feature = "async")]
pub mod stream;

pub use decode::*;
pub use key::*;
pub use mouse::*;
pub use source::*;
#[cfg(feature = "async")]
pub use stream::EventStream;

use crate::ansi::mode::{Mode, ModeSetting};
use crate::color::Color;
use crate::terminal::size::Winsize;

/// Which system clipboard selection an OSC 52 event refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClipboardSelection {
    /// `c` — system clipboard.
    System,
    /// `p` — primary (X11 PRIMARY) selection.
    Primary,
    /// Any other / unknown selection character.
    Other(char),
}

/// Decoded modifyOtherKeys mode (`CSI > 4 ; n m`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModifyOtherKeysMode {
    /// Disabled.
    Disabled,
    /// Mode 1 — only modify keys that don't otherwise have an xterm sequence.
    Mode1,
    /// Mode 2 — modify all keys.
    Mode2,
}

impl ModifyOtherKeysMode {
    pub fn from_value(v: u8) -> Self {
        match v {
            1 => Self::Mode1,
            2 => Self::Mode2,
            _ => Self::Disabled,
        }
    }
}

/// Reported terminal color scheme (DEC mode 2031).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorScheme {
    /// Dark mode (`CSI ? 997 ; 1 n`).
    Dark,
    /// Light mode (`CSI ? 997 ; 2 n`).
    Light,
}

impl std::fmt::Display for ColorScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ColorScheme::Dark => "dark",
            ColorScheme::Light => "light",
        })
    }
}

/// A terminal event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    // -- Input ---------------------------------------------------------------
    /// Key was pressed.
    KeyPress(Key),
    /// Key auto-repeated (Kitty Keyboard Protocol).
    KeyRepeat(Key),
    /// Key was released (Kitty Keyboard Protocol).
    KeyRelease(Key),
    /// Mouse button was pressed.
    MouseClick(Mouse),
    /// Mouse button was released.
    MouseRelease(Mouse),
    /// Mouse wheel scrolled.
    MouseWheel(Mouse),
    /// Mouse moved (with or without a button held).
    MouseMove(Mouse),

    // -- Size / geometry -----------------------------------------------------
    /// The terminal surface changed size. Emitted only for genuine
    /// change notifications: kernel SIGWINCH (full `Winsize`),
    /// `ReadConsoleInput` window-buffer-size events on Windows
    /// (cells only), and in-band CSI 48 t reports under mode 2048
    /// (full `Winsize`). Replies to explicit size queries are
    /// delivered as `WindowCellSize` / `WindowPixelSize` /
    /// `CellPixelSize` instead.
    Resize(Winsize),
    /// Reply to a `CSI 18 t` query — window size in cells.
    WindowCellSize { width: u16, height: u16 },
    /// Reply to a `CSI 14 t` query — window size in pixels.
    WindowPixelSize { width: u16, height: u16 },
    /// Reply to a `CSI 16 t` query — single-cell size in pixels.
    CellPixelSize { width: u16, height: u16 },

    // -- Focus / paste -------------------------------------------------------
    /// Focus gained.
    FocusIn,
    /// Focus lost.
    FocusOut,
    /// Bracketed paste started.
    PasteStart,
    /// Bracketed paste ended.
    PasteEnd,
    /// Streaming chunk of pasted bytes emitted between [`Event::PasteStart`]
    /// and [`Event::PasteEnd`]. Pastes that exceed the source's read
    /// buffer are split across multiple chunks; reassembly and any
    /// text decoding are the caller's responsibility (terminals may
    /// paste arbitrary binary content, not just valid UTF-8).
    PasteChunk(Vec<u8>),

    // -- Position / device attrs --------------------------------------------
    /// Cursor position report (CPR). Coordinates are zero-based (the
    /// 1-based wire form is normalized when parsed).
    CursorPosition(crate::layout::Position),
    /// Primary device attributes (DA1) — list of decoded numeric attributes.
    PrimaryDeviceAttributes(Vec<Option<u32>>),
    /// Secondary device attributes (DA2).
    SecondaryDeviceAttributes(Vec<Option<u32>>),
    /// Tertiary device attributes (DA3) — terminal ID string.
    TertiaryDeviceAttributes(String),
    /// Terminal version reply (XTVERSION / OSC).
    TerminalVersion(String),

    // -- Mode / capability reports ------------------------------------------
    /// DECRPM / RM mode report.
    ModeReport { mode: Mode, setting: ModeSetting },
    /// modifyOtherKeys report.
    ModifyOtherKeys(ModifyOtherKeysMode),
    /// Kitty keyboard protocol active-enhancements report
    /// (`CSI ? <flags> u`). The payload is the parsed
    /// [`crate::ansi::KittyKeyboardFlags`] bitset.
    KittyKeyboardEnhancements(crate::ansi::KittyKeyboardFlags),
    /// XTWINOPS reply (window operation).
    WindowOp { op: u32, args: Vec<Option<u32>> },
    /// XTGETTCAP / termcap capability reply.
    Termcap(String),

    // -- Colors --------------------------------------------------------------
    /// OSC 10 default foreground color reply.
    ForegroundColor(Color),
    /// OSC 11 default background color reply.
    BackgroundColor(Color),
    /// OSC 12 cursor color reply.
    CursorColor(Color),
    /// Color scheme is dark (DEC 2031 report).
    DarkColorScheme,
    /// Color scheme is light (DEC 2031 report).
    LightColorScheme,

    // -- Clipboard / graphics ------------------------------------------------
    /// OSC 52 clipboard content reply.
    Clipboard {
        selection: ClipboardSelection,
        content: String,
    },
    /// Kitty graphics response (APC `G ...` payload).
    KittyGraphics {
        options: Vec<(String, String)>,
        payload: Vec<u8>,
    },

    // -- Group / unknown -----------------------------------------------------
    /// Multiple events emitted by a single sequence.
    Multi(Vec<Event>),
    /// Unknown CSI sequence (parameters + intermediates + final byte).
    UnknownCsi(Vec<u8>),
    /// Unknown SS3 sequence.
    UnknownSs3(Vec<u8>),
    /// Unknown OSC sequence (payload bytes, no ESC/ST framing).
    UnknownOsc(Vec<u8>),
    /// Unknown DCS sequence (payload).
    UnknownDcs(Vec<u8>),
    /// Unknown SOS sequence (payload).
    UnknownSos(Vec<u8>),
    /// Unknown PM sequence (payload).
    UnknownPm(Vec<u8>),
    /// Unknown APC sequence (payload).
    UnknownApc(Vec<u8>),
    /// Catch-all for unrecognized byte sequences.
    Unknown(Vec<u8>),
}

impl Event {
    /// Borrow the [`Key`] payload if this is any key event
    /// ([`Event::KeyPress`], [`Event::KeyRepeat`], or
    /// [`Event::KeyRelease`]).
    pub fn as_key(&self) -> Option<&Key> {
        match self {
            Event::KeyPress(k) | Event::KeyRepeat(k) | Event::KeyRelease(k) => Some(k),
            _ => None,
        }
    }

    /// Borrow the [`Mouse`] payload if this is any mouse event
    /// ([`Event::MouseClick`], [`Event::MouseRelease`],
    /// [`Event::MouseWheel`], or [`Event::MouseMove`]).
    pub fn as_mouse(&self) -> Option<&Mouse> {
        match self {
            Event::MouseClick(m)
            | Event::MouseRelease(m)
            | Event::MouseWheel(m)
            | Event::MouseMove(m) => Some(m),
            _ => None,
        }
    }
}

/// Reassemble streaming [`Event::PasteChunk`] payloads back into a
/// single owned buffer.
///
/// Bracketed pastes are emitted as a sequence of `PasteChunk(Vec<u8>)`
/// events bracketed by [`Event::PasteStart`] and [`Event::PasteEnd`].
/// Callers that want the whole paste as one value push every chunk's
/// bytes into a `PasteBuffer` and call [`PasteBuffer::into_string`] (or
/// [`PasteBuffer::into_bytes`]) once `PasteEnd` arrives.
#[derive(Debug, Default, Clone)]
pub struct PasteBuffer {
    buf: Vec<u8>,
}

impl PasteBuffer {
    /// Create an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append the bytes of one `PasteChunk` payload.
    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Total bytes accumulated so far.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether no bytes have been pushed yet.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Borrow the accumulated bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Consume the accumulator and return the raw bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Consume the accumulator and decode as UTF-8, replacing invalid
    /// sequences with `U+FFFD`. Use this when paste content is expected
    /// to be text and lossy recovery is acceptable.
    pub fn into_string_lossy(self) -> String {
        match String::from_utf8(self.buf) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        }
    }

    /// Consume the accumulator and decode as UTF-8, returning the raw
    /// bytes back if the content is not valid UTF-8.
    pub fn into_string(self) -> Result<String, Vec<u8>> {
        String::from_utf8(self.buf).map_err(|e| e.into_bytes())
    }
}

/// One-shot convenience: concatenate every `PasteChunk` payload from an
/// iterator and decode as UTF-8 (lossy).
///
/// Equivalent to feeding each chunk to a [`PasteBuffer`] and calling
/// [`PasteBuffer::into_string_lossy`].
pub fn decode_paste_chunks<'a, I>(chunks: I) -> String
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let mut buf = PasteBuffer::new();
    for c in chunks {
        buf.push(c);
    }
    buf.into_string_lossy()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_buffer_concatenates_chunks() {
        let mut b = PasteBuffer::new();
        assert!(b.is_empty());
        b.push(b"hello ");
        b.push(b"world");
        assert_eq!(b.len(), 11);
        assert_eq!(b.as_bytes(), b"hello world");
        assert_eq!(b.into_string().unwrap(), "hello world");
    }

    #[test]
    fn color_scheme_display() {
        assert_eq!(ColorScheme::Dark.to_string(), "dark");
        assert_eq!(ColorScheme::Light.to_string(), "light");
    }

    #[test]
    fn paste_buffer_into_string_lossy_replaces_invalid_utf8() {
        let mut b = PasteBuffer::new();
        b.push(b"ok \xff bad");
        let s = b.into_string_lossy();
        assert!(s.contains("ok "));
        assert!(s.contains("bad"));
        assert!(s.contains("\u{FFFD}"));
    }

    #[test]
    fn paste_buffer_into_string_strict_returns_bytes_on_invalid() {
        let mut b = PasteBuffer::new();
        b.push(b"\xff\xfe");
        let err = b.into_string().unwrap_err();
        assert_eq!(err, vec![0xff, 0xfe]);
    }

    #[test]
    fn paste_buffer_reassembles_split_codepoint() {
        // The 4-byte emoji split across two chunks must reassemble
        // cleanly when decoded after all chunks are pushed.
        let bytes = "📺".as_bytes();
        let (a, b) = bytes.split_at(2);
        let mut buf = PasteBuffer::new();
        buf.push(a);
        buf.push(b);
        assert_eq!(buf.into_string().unwrap(), "📺");
    }

    #[test]
    fn decode_paste_chunks_helper() {
        let chunks: Vec<&[u8]> = vec![b"foo ", b"bar ", b"baz"];
        assert_eq!(decode_paste_chunks(chunks), "foo bar baz");
    }
}

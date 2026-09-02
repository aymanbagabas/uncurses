//! Terminal events and event-stream decoding.
//!
//! This module owns the core [`Event`] enum together with the
//! internal decoder that parses raw terminal bytes into events, the
//! platform-specific [`EventSource`] that drives the decoder from a
//! tty, and the key/mouse types that events carry.
//!
//! ## The decode pipeline
//!
//! Input arrives as a raw byte stream. The source reads bytes (waking on a
//! self-pipe so another thread can interrupt a blocking read), feeds them to
//! the decoder, and hands back fully-formed [`Event`] values. Escape
//! sequences may straddle reads, so the decoder buffers partial input and a
//! short timeout disambiguates a lone `Esc` key from the start of a CSI/SS3
//! sequence.
//!
//! ```text
//!   tty input        EventSource              Decoder            caller
//!   ─────────        ───────────              ───────            ──────
//!   bytes  ───────▶  read + buffer  ───────▶  scan sequences ─▶  Event
//!     │                   ▲                        │
//!     │                   └── Esc-timeout ◀────────┘ (Esc key vs CSI/SS3?)
//!     └── self-pipe wake ─┘  (interrupt a blocking read from another thread)
//! ```
//!
//! Build an [`EventSource`] over a terminal's input half and read typed
//! events in a loop. Keys parse from strings and compare by canonical
//! chord, so matching a shortcut is plain equality.
//!
//! ```no_run
//! use uncurses::event::{Event, EventSource, Key};
//! use uncurses::terminal::Terminal;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut term = Terminal::stdio();
//! term.make_raw()?;
//! let mut events = EventSource::new(term.input())?;
//!
//! let quit: Key = "ctrl+c".parse().unwrap();
//! loop {
//!     match events.read()? {
//!         Event::KeyPress(ref k) if *k == quit => break,
//!         Event::KeyPress(k) => { let _ = k.code; }
//!         Event::Resize(ws) => { let _ = (ws.col, ws.row); }
//!         _ => {}
//!     }
//! }
//! term.restore()
//! # }
//! ```
//!
//! ## Queries
//!
//! To ask the terminal a question (its background color, cell size,
//! device attributes, and so on), write the request bytes from the
//! [`ansi`](crate::ansi) module to the output and read the matching reply
//! event back through the same source. The
//! [`Screen`](crate::screen::Screen) facade wraps this in `request_*` methods
//! whose replies surface as ordinary events, never swallowing the user's
//! keystrokes in between.
//!
//! ## Async
//!
//! With the `async` feature, `EventStream` reads the same events through a
//! [`futures_core::Stream`], so the loop becomes `while let Some(ev) =
//! stream.next().await`.
//!
//! [`futures_core::Stream`]: https://docs.rs/futures-core/latest/futures_core/stream/trait.Stream.html

pub(crate) mod decode;
#[cfg(test)]
mod decode_safety_tests;
mod key;
mod mouse;
mod pending;
pub(crate) mod poll;
mod sigwinch;
mod source;
#[cfg(unix)]
mod source_unix;
#[cfg(windows)]
mod source_windows;
#[cfg(feature = "async")]
mod stream;

pub use key::{Key, KeyCode, KeyModifiers, ParseKeyError};
pub use mouse::{Mouse, MouseButton, mouse_pixel_to_cell};
pub use source::{DEFAULT_ESC_TIMEOUT, DEFAULT_PASTE_IDLE_TIMEOUT, EventSource, Input, Waker};
#[cfg(feature = "async")]
pub use stream::EventStream;

use crate::ansi::mode::{Mode, ModeSetting};
use crate::color::Color;
use crate::terminal::Winsize;

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
    /// Convert a report value into a modifyOtherKeys mode.
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

/// Reported terminal visibility (DEC mode 2033).
///
/// This is an advisory, deliberately conservative hint used to skip expensive
/// rendering that nobody can see. [`Hidden`](Visibility::Hidden) is precise:
/// the terminal has positive knowledge that the view is not observable.
/// [`Visible`](Visibility::Visible) only means it *may* be observable.
///
/// Only `1` and `2` decode to a report; any other value is left as
/// [`Event::UnknownCsi`]. Treat a terminal that reports nothing, or reports
/// something unrecognized, as visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Visibility {
    /// Potentially visible (`CSI ? 999 ; 1 n`). The view may be observable.
    ///
    /// This does not promise that any cell is onscreen: the terminal reports
    /// it whenever visibility is unknown or any view may be observed.
    Visible,
    /// Not visible (`CSI ? 999 ; 2 n`). The terminal knows the view is not
    /// ordinarily observable, so expensive visual updates can be paused.
    ///
    /// Never assume this lasts for any minimum duration.
    Hidden,
}

impl Visibility {
    /// Whether the terminal view may be observable, so visual work is worth
    /// doing. `true` for [`Visible`](Visibility::Visible).
    pub fn is_visible(self) -> bool {
        matches!(self, Visibility::Visible)
    }
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Visibility::Visible => "visible",
            Visibility::Hidden => "hidden",
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
    WindowCellSize {
        /// Width in terminal cells.
        width: u16,
        /// Height in terminal cells.
        height: u16,
    },
    /// Reply to a `CSI 14 t` query — window size in pixels.
    WindowPixelSize {
        /// Width in pixels.
        width: u16,
        /// Height in pixels.
        height: u16,
    },
    /// Reply to a `CSI 16 t` query — single-cell size in pixels.
    CellPixelSize {
        /// Cell width in pixels.
        width: u16,
        /// Cell height in pixels.
        height: u16,
    },

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
    /// Terminal name reply (XTVERSION). Carries the raw identifier string,
    /// which typically combines a name and version (e.g. `"XTerm(380)"`).
    TerminalName(String),

    // -- Mode / capability reports ------------------------------------------
    /// DECRPM / RM mode report.
    ///
    /// The `setting` distinguishes all five DECRPM states. A terminal can
    /// report a mode as permanently set or permanently reset, meaning it
    /// recognizes the mode but will not let the host toggle it. When deciding
    /// whether a feature is usable, prefer
    /// [`ModeSetting::is_available`](crate::ansi::mode::ModeSetting::is_available)
    /// over [`is_recognized`](crate::ansi::mode::ModeSetting::is_recognized): a
    /// permanently reset mode is recognized yet can never be enabled.
    ModeReport {
        /// Reported mode.
        mode: Mode,
        /// Current mode setting.
        setting: ModeSetting,
    },
    /// modifyOtherKeys report.
    ModifyOtherKeys(ModifyOtherKeysMode),
    /// Kitty keyboard protocol active-enhancements report
    /// (`CSI ? <flags> u`). The payload is the parsed
    /// [`crate::ansi::kitty::KittyKeyboardFlags`] bitset.
    KittyKeyboardEnhancements(crate::ansi::kitty::KittyKeyboardFlags),
    /// XTWINOPS reply (window operation).
    WindowOp {
        /// Window operation number.
        op: u32,
        /// Window operation arguments.
        args: Vec<Option<u32>>,
    },
    /// XTGETTCAP / termcap capability reply. `recognized` is `true` for a
    /// successful reply (`DCS 1 + r`) and `false` for a failure
    /// (`DCS 0 + r`); the entries are decoded the same way in both cases (a
    /// failure echoes the requested, now known-unsupported, capability
    /// names).
    Termcap {
        /// Whether the requested capabilities were recognized.
        recognized: bool,
        /// Decoded `(name, value)` pairs. The value is `None` when the entry
        /// carried no `=`: either a boolean capability, reported as a bare
        /// name, or a failure reply echoing a name it does not support.
        /// `recognized` is what tells those apart.
        ///
        /// Kept as pairs because only the hex wire form is delimiter-safe:
        /// decoded values commonly contain `;` and `=` (`kf13` is
        /// `\E[1;2P`), so a joined string could not be split back apart.
        entries: Vec<(String, Option<String>)>,
    },
    /// DECRPSS setting report (`DCS 1 $ r` on success, `DCS 0 $ r` on
    /// failure), reporting a current setting such as the active SGR
    /// attributes or cursor style. Sent in answer to
    /// [`write_decrqss`](crate::ansi::status::write_decrqss).
    SettingReport(SettingReport),

    // -- Colors --------------------------------------------------------------
    /// OSC 10 default foreground color reply.
    ForegroundColor(Color),
    /// OSC 11 default background color reply.
    BackgroundColor(Color),
    /// OSC 12 cursor color reply.
    CursorColor(Color),
    /// OSC 4 indexed palette color reply (`OSC 4 ; index ; color`).
    PaletteColor {
        /// Palette color index.
        index: u8,
        /// Reported palette color.
        color: Color,
    },
    /// Color-scheme report (DEC mode 2031): whether the terminal is in its
    /// dark or light scheme. Indicates only the dark/light preference, not
    /// the actual colors.
    ColorScheme(ColorScheme),

    /// Terminal visibility report (DEC mode 2033): whether the terminal view
    /// may be observed. Arrives unsolicited while
    /// [`Program::enable_visibility_reports`] is active, and as the reply to
    /// [`Program::request_visibility`].
    ///
    /// This is independent of focus: focus says which view receives keyboard
    /// input, visibility says whether output can be seen.
    ///
    /// [`Program::enable_visibility_reports`]: crate::program::Program::enable_visibility_reports
    /// [`Program::request_visibility`]: crate::program::Program::request_visibility
    Visibility(Visibility),

    // -- Clipboard / graphics ------------------------------------------------
    /// OSC 52 clipboard content reply.
    Clipboard {
        /// Clipboard selection that was reported.
        selection: ClipboardSelection,
        /// Clipboard content.
        content: String,
    },
    /// Kitty graphics response (APC `G ...` payload).
    KittyGraphics {
        /// Response options.
        options: Vec<(String, String)>,
        /// Response payload bytes.
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

/// The payload of a DECRPSS reply, [`Event::SettingReport`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SettingReport {
    /// The terminal did not recognize the requested setting (`DCS 0 $ r ST`).
    /// The reply carries no data at all, so only the request that provoked it
    /// says which setting was turned down.
    Unrecognized,
    /// The cursor style the terminal is using, from a DECRPSS reply
    /// reporting its `DECSCUSR` setting. Ask for it with
    /// [`Program::request_cursor_style`](crate::program::Program::request_cursor_style).
    CursorStyle(crate::ansi::cursor::CursorStyle),
    /// The setting as the terminal spelled it, for a control function this
    /// library does not decode: the whole CSI sequence without its
    /// introducer, so `0;1m` for SGR or `>4;2m` for xterm's `XTQMODKEYS`.
    Raw(String),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_scheme_display() {
        assert_eq!(ColorScheme::Dark.to_string(), "dark");
        assert_eq!(ColorScheme::Light.to_string(), "light");
    }

    #[test]
    fn visibility_display_and_predicate() {
        assert_eq!(Visibility::Visible.to_string(), "visible");
        assert_eq!(Visibility::Hidden.to_string(), "hidden");
        assert!(Visibility::Visible.is_visible());
        assert!(!Visibility::Hidden.is_visible());
    }
}

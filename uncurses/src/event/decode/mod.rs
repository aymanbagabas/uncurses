//! Byte-stream decoder for terminal events.
//!
//! ## Purpose
//!
//! [`Decoder`] translates terminal input bytes into [`Event`] values. It knows
//! the byte grammar for C0 controls, UTF-8 characters, 7-bit and 8-bit escape
//! sequence introducers, bracketed paste, mouse protocols, keyboard extensions,
//! terminal query replies, and Windows input-mode packets.
//!
//! ```text
//! bytes ──▶ ground byte dispatch
//!   │        ├─ UTF-8 / C0 controls ─────▶ KeyPress
//!   │        └─ ESC or C1 introducer ─┬──▶ CSI / SS3 / OSC / DCS / APC
//!   │                                  └──▶ Alt-key or pending ESC
//!   └─ paste mode ───────────────────────▶ PasteChunk … PasteEnd
//! ```
//!
//! ## Key types
//!
//! * [`Decoder`] owns the state that must survive across feeds: an internal
//!   buffer for [`Decoder::parse`], bracketed-paste state, queued multi-event
//!   expansions, UTF-8 mouse mode, and Windows surrogate/modifier state.
//! * [`DecoderFlags`] selects a few ambiguous legacy interpretations such as
//!   Tab versus `Ctrl+i` and Backspace versus Delete.
//! * [`Event`] carries the decoded result; unknown framed strings are preserved
//!   as `Unknown*` variants instead of being silently dropped.
//!
//! ## APIs
//!
//! Use [`Decoder::parse`] when the decoder should retain incomplete bytes
//! between calls. Use [`Decoder::parse_one`] when an outer owner, such as
//! [`EventSource`](crate::event::EventSource), owns the buffer and wants a
//! `(consumed, event)` result for exactly one event at a time.
//!
//! ## Gotchas
//!
//! Escape-timeout policy is intentionally outside the normal parse path. A lone
//! `ESC` or incomplete C1 introducer returns incomplete until the caller decides
//! the deadline has expired, then calls [`Decoder::drain`] or the source-internal
//! expiry path. During bracketed paste, timeout disambiguation is suspended and
//! bytes are streamed as raw [`Event::PasteChunk`] payloads.
use super::key::{Key, KeyCode, KeyModifiers};
mod apc;
mod csi;
mod dcs;
mod escape;
mod flags;
mod kitty;
mod osc;
mod paste;
mod result;
mod sos_pm;
mod ss3;
mod utf8;
mod util;
mod win32;
use super::Event;
pub use flags::DecoderFlags;
use result::ParseResult;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
pub(crate) use util::is_c1_introducer;

/// Stateful byte parser that produces [`Event`] values.
///
/// `Decoder` can either own its pending bytes through [`Decoder::parse`] or be
/// driven by an external buffer through [`Decoder::parse_one`]. It tracks state
/// that is meaningful across calls: bracketed paste mode, pending events from a
/// sequence that expands to several events, UTF-8 mouse mode, and Windows input
/// surrogate/modifier state.
///
/// The decoder does not perform I/O and does not implement wall-clock timeout
/// policy by itself. Callers that need Escape-key disambiguation must call
/// [`Decoder::drain`] or use [`EventSource`](crate::event::EventSource), which
/// applies the timeout around [`Decoder::parse_one`].
pub struct Decoder {
    buf: Vec<u8>,
    /// Decoder behavior flags (disambiguation toggles for legacy keys).
    pub(super) flags: DecoderFlags,
    /// True while we are between `Event::PasteStart` and `Event::PasteEnd`.
    pub(super) in_paste: bool,
    /// When `true`, raw mouse coordinates after `CSI M` are decoded as UTF-8
    /// codepoints (mode 1005) instead of single bytes.
    utf8_mouse: bool,
    /// When `true`, the parse loop treats any `ParseResult::Incomplete` as a
    /// timed-out partial sequence and resolves it to best-effort events
    /// (typically a bare Escape key followed by the remaining bytes).
    expired: bool,
    /// Events queued by sequences that expand into more than one event
    /// (e.g. a win32-input-mode key with `wRepeatCount > 1`). These are
    /// drained before consuming any new bytes.
    pub(super) pending: RefCell<VecDeque<Event>>,
    /// Last-seen control-key state from the win32-input-mode stream. Used
    /// to recover left/right modifier identity on key-release records.
    pub(super) win32_last_cks: Cell<u32>,
    /// High UTF-16 surrogate buffered from a `vk == 0` win32-input-mode
    /// record, indexed by `bKeyDown` (0 = release, 1 = press).
    pub(super) win32_high_surrogate: Cell<[Option<u16>; 2]>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new(DecoderFlags::empty())
    }
}

impl Decoder {
    /// Construct a decoder with the given behavior flags.
    ///
    /// `flags` chooses the preferred interpretation for a few ambiguous legacy
    /// input bytes. The decoder starts outside paste mode, with UTF-8 mouse mode
    /// disabled and no buffered input. Construction allocates the internal
    /// `parse` buffer but performs no I/O and never panics.
    pub fn new(flags: DecoderFlags) -> Self {
        Self {
            buf: Vec::with_capacity(256),
            flags,
            in_paste: false,
            utf8_mouse: false,
            expired: false,
            pending: RefCell::new(VecDeque::new()),
            win32_last_cks: Cell::new(0),
            win32_high_surrogate: Cell::new([None, None]),
        }
    }

    /// Returns `true` when the parser holds an unfinished escape sequence
    /// (a buffered partial input that begins with `ESC` or with one of the
    /// 8-bit C1 sequence introducers, and is not part of an active bracketed
    /// paste).
    ///
    /// Callers can use this to drive an escape-sequence timeout: if no
    /// further bytes arrive before the timeout elapses, call
    /// [`Decoder::drain`] to resolve the partial sequence.
    pub fn has_pending(&self) -> bool {
        if self.buf.is_empty() || self.in_paste {
            return false;
        }
        let b0 = self.buf[0];
        b0 == 0x1b || is_c1_introducer(b0)
    }

    /// Force-drain any buffered partial escape sequence as best-effort events.
    ///
    /// A leading `ESC` byte is emitted as [`KeyCode::Escape`] by this legacy
    /// buffered drain path; the remaining bytes are then re-parsed normally (so
    /// e.g. `ESC '['` becomes an `Esc` keypress followed by a `Char('[')`
    /// keypress). Source-driven timeout expiry uses a separate leading-byte
    /// helper that can honor [`DecoderFlags::CTRL_OPEN_BRACKET`].
    ///
    /// While a bracketed paste is in progress this is a no-op — paste content
    /// is allowed to span arbitrary time.
    pub fn drain(&mut self) -> Vec<Event> {
        if self.in_paste || self.buf.is_empty() {
            return Vec::new();
        }
        self.expired = true;
        let events = self.parse(&[]);
        self.expired = false;
        events
    }

    /// Enable or disable UTF-8 mouse decoding (xterm mode 1005).
    ///
    /// When enabled, X10-style `CSI M` mouse reports read their three values as
    /// UTF-8 codepoints instead of raw bytes. This setting only affects future
    /// parses and does not modify already-buffered bytes.
    pub fn set_utf8_mouse(&mut self, enabled: bool) {
        self.utf8_mouse = enabled;
    }

    /// Return whether UTF-8 mouse decoding (xterm mode 1005) is enabled.
    ///
    /// Reading this flag has no side effects and performs no parsing.
    pub fn utf8_mouse(&self) -> bool {
        self.utf8_mouse
    }

    /// Slice-driven parse — pulls **one** event from `data` and returns
    /// `(consumed, event)`.
    ///
    /// * `(n, Some(event))` — `event` was decoded; the caller should
    ///   advance its buffer past the first `n` bytes.
    /// * `(0, None)` — the buffer holds a partial sequence; the caller
    ///   should keep the bytes and retry after reading more input.
    /// * `(n, None)` with `n > 0` — `n` bytes were consumed but produced
    ///   no user-facing event (e.g. an invalid CSI intermediate byte or a
    ///   malformed UTF-8 byte).
    ///
    /// The parser does **not** retain `data` between calls. Any unconsumed
    /// bytes remain the caller's responsibility. Inside a bracketed
    /// paste, paste content streams as [`Event::PasteChunk`] — large
    /// pastes split across multiple calls naturally.
    ///
    /// Escape-sequence timeout is **not** handled here; if `data`
    /// begins with `0x1B` and no continuation byte is available, this
    /// returns `(0, None)`. The caller (typically an `EventSource`)
    /// must apply its own timeout policy and synthesise a bare
    /// [`KeyCode::Escape`] when desired.
    pub fn parse_one(&mut self, data: &[u8]) -> (usize, Option<Event>) {
        if let Some(evt) = self.pending.borrow_mut().pop_front() {
            return (0, Some(evt));
        }
        if data.is_empty() {
            return (0, None);
        }

        if self.in_paste {
            return self.parse_paste_chunk(data);
        }

        match self.try_parse(data) {
            ParseResult::Event(event, consumed) => {
                if matches!(event, Event::PasteStart) {
                    self.in_paste = true;
                }
                match event {
                    Event::Multi(group) => {
                        let mut it = group.into_iter();
                        let first = it.next();
                        for e in it {
                            self.pending.borrow_mut().push_back(e);
                        }
                        (consumed, first)
                    }
                    other => (consumed, Some(other)),
                }
            }
            ParseResult::Incomplete => (0, None),
            ParseResult::None(consumed) => {
                if consumed > 0 {
                    (consumed, None)
                } else {
                    let bytes = &data[..1];
                    (1, Some(Event::Unknown(bytes.to_vec())))
                }
            }
        }
    }

    /// `true` while the decoder is between `PasteStart` and `PasteEnd`.
    /// Useful for embedders implementing their own watchdog or
    /// idle-timeout policy on top of `EventSource`.
    pub fn in_paste(&self) -> bool {
        self.in_paste
    }

    /// Force-exit bracketed paste mode. If the decoder was inside a
    /// paste, the flag is cleared and `Some(Event::PasteEnd)` is
    /// returned for the caller to enqueue. Returns `None` when the
    /// decoder was not in paste.
    ///
    /// Intended as an escape hatch for callers detecting a stuck
    /// paste (e.g. a malformed stream that never sent the terminator)
    /// or applying their own size cap / cooldown policy.
    pub fn end_paste(&mut self) -> Option<Event> {
        if self.in_paste {
            self.in_paste = false;
            Some(Event::PasteEnd)
        } else {
            None
        }
    }

    /// Toggle the escape-timeout flag. When `true`, the decoder
    /// commits buffered partial escape sequences as best-effort key
    /// events instead of returning `Incomplete`. Embedders driving the
    /// decoder against an external buffer (e.g. [`crate::event::EventSource`])
    /// set this before draining once the escape deadline elapses.
    pub(crate) fn set_expired(&mut self, value: bool) {
        self.expired = value;
    }

    /// Synthesise the timeout fallback for a leading byte that the
    /// caller has decided cannot be a partial sequence anymore.
    ///
    /// `b0` must be either `0x1B` (bare Escape key) or an 8-bit
    /// C1 introducer (Ctrl+Alt fallback). Returns the synthesised
    /// event; the caller should advance its buffer by one byte.
    pub(crate) fn expire_leading(&self, b0: u8) -> Option<Event> {
        if b0 == 0x1b {
            let key = if self.flags.contains(DecoderFlags::CTRL_OPEN_BRACKET) {
                Key::new(KeyCode::Char('['), KeyModifiers::CTRL).normalized()
            } else {
                Key::new(KeyCode::Escape, KeyModifiers::empty()).normalized()
            };
            Some(Event::KeyPress(key))
        } else if is_c1_introducer(b0) {
            // 0x80..=0x9F → '@'..'_' (Ctrl+Alt+letter convention).
            // Lowercase ASCII letters so `normalize()` doesn't
            // synthesize an extra SHIFT modifier from the uppercase
            // form; ctrl is treated case-insensitively.
            let c = ((b0 - 0x40) as char).to_ascii_lowercase();
            Some(Event::KeyPress(
                Key::new(KeyCode::Char(c), KeyModifiers::CTRL | KeyModifiers::ALT).normalized(),
            ))
        } else {
            None
        }
    }

    /// Feed bytes into the decoder-owned buffer and return all complete events.
    ///
    /// Any incomplete prefix is retained inside the decoder for the next call.
    /// Complete sequences may produce several events, and bracketed paste bodies
    /// may produce one or more [`Event::PasteChunk`] values depending on feed
    /// boundaries. Passing an empty slice is valid and can drain events queued by
    /// a previous multi-event sequence.
    ///
    /// This method never panics for malformed terminal input; unknown or invalid
    /// bytes are surfaced as unknown events or skipped according to the parser's
    /// recovery rules.
    pub fn parse(&mut self, data: &[u8]) -> Vec<Event> {
        self.buf.extend_from_slice(data);
        let mut events = Vec::new();

        loop {
            // Drain any events queued by previously-parsed sequences (e.g. a
            // win32-input-mode key with a repeat count greater than one)
            // before consuming new bytes.
            if let Some(evt) = self.pending.borrow_mut().pop_front() {
                events.push(evt);
                continue;
            }
            if self.buf.is_empty() {
                break;
            }

            // While in bracketed paste, stream the bytes as PasteChunk
            // events. Assembly is the caller's responsibility.
            if self.in_paste {
                let mut scan = 0;
                let mut terminated = false;
                let mut hold_from: Option<usize> = None;
                while scan < self.buf.len() {
                    if self.buf[scan] != 0x1B {
                        scan += 1;
                        continue;
                    }
                    match self.try_parse(&self.buf[scan..]) {
                        ParseResult::Event(Event::PasteEnd, consumed) => {
                            if scan > 0 {
                                events.push(Event::PasteChunk(self.buf[..scan].to_vec()));
                            }
                            self.buf.drain(..scan + consumed);
                            self.in_paste = false;
                            events.push(Event::PasteEnd);
                            terminated = true;
                            break;
                        }
                        ParseResult::Incomplete => {
                            hold_from = Some(scan);
                            break;
                        }
                        ParseResult::Event(_, consumed) => {
                            scan += consumed.max(1);
                        }
                        ParseResult::None(consumed) => {
                            scan += consumed.max(1);
                        }
                    }
                }
                if terminated {
                    continue;
                }
                let take = hold_from.unwrap_or(self.buf.len());
                if take > 0 {
                    events.push(Event::PasteChunk(self.buf[..take].to_vec()));
                    self.buf.drain(..take);
                }
                break;
            }

            match self.try_parse(&self.buf) {
                ParseResult::Event(event, consumed) => {
                    let is_paste_start = matches!(event, Event::PasteStart);
                    // Flatten `Event::Multi` into individual events so callers
                    // that match by enum variant don't miss anything.
                    match event {
                        Event::Multi(group) => {
                            for e in group {
                                events.push(e);
                            }
                        }
                        other => events.push(other),
                    }
                    self.buf.drain(..consumed);
                    if is_paste_start {
                        self.in_paste = true;
                    }
                }
                ParseResult::Incomplete => {
                    if self.expired {
                        // Timeout reached: resolve the partial sequence as
                        // best-effort. A leading ESC becomes a standalone
                        // Escape keypress; a leading 8-bit C1 introducer
                        // becomes its Ctrl+Alt+<code-0x40> fallback; anything
                        // else is reported as Unknown so no bytes are
                        // silently dropped.
                        let b0 = self.buf[0];
                        if b0 == 0x1b {
                            events.push(Event::KeyPress(
                                Key::new(KeyCode::Escape, KeyModifiers::empty()).normalized(),
                            ));
                            self.buf.drain(..1);
                            continue;
                        }
                        if is_c1_introducer(b0) {
                            // See `expire_leading`: lowercase ASCII
                            // letters so SHIFT is not synthesized for
                            // the Ctrl+Alt+letter fallback.
                            let c = ((b0 - 0x40) as char).to_ascii_lowercase();
                            events.push(Event::KeyPress(
                                Key::new(KeyCode::Char(c), KeyModifiers::CTRL | KeyModifiers::ALT)
                                    .normalized(),
                            ));
                            self.buf.drain(..1);
                            continue;
                        }
                        events.push(Event::Unknown(self.buf.clone()));
                        self.buf.clear();
                    }
                    break;
                }
                ParseResult::None(consumed) => {
                    if consumed > 0 {
                        self.buf.drain(..consumed);
                    } else {
                        let byte = self.buf[0];
                        events.push(Event::Unknown(vec![byte]));
                        self.buf.drain(..1);
                    }
                }
            }
        }

        events
    }

    fn try_parse(&self, buf: &[u8]) -> ParseResult {
        if buf.is_empty() {
            return ParseResult::Incomplete;
        }

        match buf[0] {
            0x1b => self.parse_escape(buf),
            0x01..=0x08 | 0x0b..=0x0c | 0x0e..=0x1a => {
                // Ctrl+A through Ctrl+Z (excluding Tab/LF/CR/Esc which have dedicated keys).
                let c = (buf[0] - 1 + b'a') as char;
                ParseResult::Event(
                    Event::KeyPress(Key::new(KeyCode::Char(c), KeyModifiers::CTRL).normalized()),
                    1,
                )
            }
            0x09 => {
                if self.flags.contains(DecoderFlags::CTRL_I) {
                    ParseResult::Event(
                        Event::KeyPress(
                            Key::new(KeyCode::Char('i'), KeyModifiers::CTRL).normalized(),
                        ),
                        1,
                    )
                } else {
                    ParseResult::Event(
                        Event::KeyPress(Key::new(KeyCode::Tab, KeyModifiers::empty()).normalized()),
                        1,
                    )
                }
            }
            0x0a => ParseResult::Event(
                Event::KeyPress(Key::new(KeyCode::Enter, KeyModifiers::empty()).normalized()),
                1,
            ),
            0x0d => {
                if self.flags.contains(DecoderFlags::CTRL_M) {
                    ParseResult::Event(
                        Event::KeyPress(
                            Key::new(KeyCode::Char('m'), KeyModifiers::CTRL).normalized(),
                        ),
                        1,
                    )
                } else {
                    ParseResult::Event(
                        Event::KeyPress(
                            Key::new(KeyCode::Enter, KeyModifiers::empty()).normalized(),
                        ),
                        1,
                    )
                }
            }
            0x00 => {
                let key = if self.flags.contains(DecoderFlags::CTRL_AT) {
                    Key::new(KeyCode::Char('@'), KeyModifiers::CTRL).normalized()
                } else {
                    Key::new(KeyCode::Space, KeyModifiers::CTRL).normalized()
                };
                ParseResult::Event(Event::KeyPress(key), 1)
            }
            // Ctrl+\, Ctrl+], Ctrl+^, Ctrl+_
            0x1c => ParseResult::Event(
                Event::KeyPress(Key::new(KeyCode::Char('\\'), KeyModifiers::CTRL).normalized()),
                1,
            ),
            0x1d => ParseResult::Event(
                Event::KeyPress(Key::new(KeyCode::Char(']'), KeyModifiers::CTRL).normalized()),
                1,
            ),
            0x1e => ParseResult::Event(
                Event::KeyPress(Key::new(KeyCode::Char('^'), KeyModifiers::CTRL).normalized()),
                1,
            ),
            0x1f => ParseResult::Event(
                Event::KeyPress(Key::new(KeyCode::Char('_'), KeyModifiers::CTRL).normalized()),
                1,
            ),
            0x7f => {
                let code = if self.flags.contains(DecoderFlags::BACKSPACE_IS_DELETE) {
                    KeyCode::Delete
                } else {
                    KeyCode::Backspace
                };
                ParseResult::Event(
                    Event::KeyPress(Key::new(code, KeyModifiers::empty()).normalized()),
                    1,
                )
            }
            // 8-bit C1 control codes that introduce a string/control sequence
            // (equivalent to their `ESC X` 7-bit forms).
            0x8f => self.parse_ss3(buf),
            0x90 => self.parse_dcs(buf),
            0x98 => self.parse_sos_pm_apc(buf, b'X'),
            0x9b => self.parse_csi(buf),
            0x9d => self.parse_osc(buf),
            0x9e => self.parse_sos_pm_apc(buf, b'^'),
            0x9f => self.parse_apc(buf),
            // Remaining C1 control codes (0x80..=0x9F) — including a stray
            // ST (0x9C) — are encoded as Ctrl+Alt+<code - 0x40>. Lowercase
            // ASCII letters so `normalize()` does not synthesize SHIFT
            // from the uppercase form.
            b @ 0x80..=0x9f => {
                let c = ((b - 0x40) as char).to_ascii_lowercase();
                ParseResult::Event(
                    Event::KeyPress(
                        Key::new(KeyCode::Char(c), KeyModifiers::CTRL | KeyModifiers::ALT)
                            .normalized(),
                    ),
                    1,
                )
            }
            b if b >= 0x80 => self.parse_utf8(buf),
            0x20 => ParseResult::Event(
                Event::KeyPress(Key::new(KeyCode::Space, KeyModifiers::empty()).normalized()),
                1,
            ),
            b => ParseResult::Event(
                Event::KeyPress(
                    Key::new(KeyCode::Char(b as char), KeyModifiers::empty()).normalized(),
                ),
                1,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::event::ClipboardSelection;
    use crate::event::ColorScheme;

    #[test]
    fn test_parse_simple_char() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"a");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Char('a'));
            }
            _ => panic!("Expected Key event"),
        }
    }

    #[test]
    fn test_parse_ctrl_c() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(&[0x03]);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Char('c'));
                assert!(k.modifiers.contains(KeyModifiers::CTRL));
            }
            _ => panic!("Expected Key event"),
        }
    }

    #[test]
    fn test_parse_arrow_up() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[A");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => assert_eq!(k.code, KeyCode::Up),
            _ => panic!("Expected Key event"),
        }
    }

    #[test]
    fn test_parse_shift_arrow() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[1;2A");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Up);
                assert!(k.modifiers.contains(KeyModifiers::SHIFT));
            }
            _ => panic!("Expected Key event"),
        }
    }

    #[test]
    fn test_parse_f5() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[15~");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => assert_eq!(k.code, KeyCode::F(5)),
            _ => panic!("Expected Key event"),
        }
    }

    #[test]
    fn test_parse_sgr_mouse() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[<0;10;20M");
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::MouseClick(_)));
    }

    #[test]
    fn test_parse_focus() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[I");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], Event::FocusIn);

        let events = parser.parse(b"\x1b[O");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], Event::FocusOut);
    }

    #[test]
    fn test_parse_color_scheme_report() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        assert_eq!(
            parser.parse(b"\x1b[?997;1n"),
            vec![Event::ColorScheme(ColorScheme::Dark)]
        );
        assert_eq!(
            parser.parse(b"\x1b[?997;2n"),
            vec![Event::ColorScheme(ColorScheme::Light)]
        );
        // Unknown sub-report value: branch returns None; consumer may
        // see a fallthrough Unknown event but never a ColorScheme.
        let evs = parser.parse(b"\x1b[?997;9n");
        assert!(!evs.iter().any(|e| matches!(e, Event::ColorScheme(_))));
        // Wrong primary param is not a color scheme report.
        let evs = parser.parse(b"\x1b[?996;1n");
        assert!(!evs.iter().any(|e| matches!(e, Event::ColorScheme(_))));
        // Wrong number of params (1 or 3) is rejected.
        let evs = parser.parse(b"\x1b[?997n");
        assert!(!evs.iter().any(|e| matches!(e, Event::ColorScheme(_))));
        let evs = parser.parse(b"\x1b[?997;1;0n");
        assert!(!evs.iter().any(|e| matches!(e, Event::ColorScheme(_))));
    }

    #[test]
    fn test_parse_cursor_position() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[5;10R");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::CursorPosition(pos) => {
                assert_eq!(pos.y, 4);
                assert_eq!(pos.x, 9);
            }
            _ => panic!("Expected CursorPosition"),
        }
    }

    #[test]
    fn test_parse_in_band_resize_with_pixels() {
        // CSI 48 ; rows ; cols ; ypix ; xpix t — full five-param form.
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[48;30;100;480;800t");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Resize(ws) => {
                assert_eq!((ws.row, ws.col), (30, 100));
                assert_eq!((ws.ypixel, ws.xpixel), (480, 800));
            }
            other => panic!("expected Resize, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_in_band_resize_without_pixels() {
        // CSI 48 ; rows ; cols t — pixel fields omitted. Must still decode
        // as a resize (not a generic window-op) with zero pixel size.
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[48;30;100t");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Resize(ws) => {
                assert_eq!((ws.row, ws.col), (30, 100));
                assert_eq!((ws.ypixel, ws.xpixel), (0, 0));
            }
            other => panic!("expected Resize, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_csi_z_shift_tab() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[Z");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Tab);
                assert_eq!(k.modifiers, KeyModifiers::SHIFT);
            }
            _ => panic!("Expected Key event"),
        }
    }

    #[test]
    fn test_parse_alt_char() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1ba");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Char('a'));
                assert!(k.modifiers.contains(KeyModifiers::ALT));
            }
            _ => panic!("Expected Key event"),
        }
    }

    #[test]
    fn test_parse_paste_bracketed() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        // Full paste sequence in one feed: start + content + end.
        let events = parser.parse(b"\x1b[200~hello world\x1b[201~");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], Event::PasteStart);
        assert_eq!(events[1], Event::PasteChunk(b"hello world".to_vec()));
        assert_eq!(events[2], Event::PasteEnd);
    }

    #[test]
    fn test_parse_paste_split_across_feeds() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let e1 = parser.parse(b"\x1b[200~hello ");
        assert_eq!(
            e1,
            vec![Event::PasteStart, Event::PasteChunk(b"hello ".to_vec())]
        );
        let e2 = parser.parse(b"world\x1b[201~");
        assert_eq!(
            e2,
            vec![Event::PasteChunk(b"world".to_vec()), Event::PasteEnd]
        );
    }

    #[test]
    fn test_parse_paste_escape_inside() {
        // ESC sequences inside paste should be preserved verbatim (no
        // re-interpretation).
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[200~a\x1b[Ab\x1b[201~");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], Event::PasteStart);
        assert_eq!(events[1], Event::PasteChunk(b"a\x1b[Ab".to_vec()));
        assert_eq!(events[2], Event::PasteEnd);
    }

    #[test]
    fn test_parse_paste_preserves_control_codes_and_sequences() {
        // A paste body that mixes:
        //   * raw control bytes (NUL, BEL, BS, HT, LF, VT, FF, CR, DEL),
        //   * a C1 introducer (0x9B),
        //   * a full CSI sequence (Up arrow),
        //   * an OSC sequence (set title),
        //   * an SS3 sequence (F1),
        //   * a nested PasteStart marker,
        //   * raw non-UTF-8 bytes (0xFF 0xFE).
        // Everything except the closing `\x1b[201~` must round-trip
        // verbatim through a single PasteChunk.
        let body: Vec<u8> = [
            b"text".as_slice(),
            b"\x00\x07\x08\x09\x0a\x0b\x0c\x0d\x7f",
            b"\x9b",          // 8-bit CSI introducer (literal)
            b"\x1b[A",        // Up arrow CSI
            b"\x1b]0;hi\x07", // OSC set-title
            b"\x1bOP",        // SS3 F1
            b"\x1b[200~",     // nested PasteStart marker (literal)
            b"\xff\xfe",      // invalid UTF-8
            b"end",
        ]
        .concat();

        let mut wire = Vec::new();
        wire.extend_from_slice(b"\x1b[200~");
        wire.extend_from_slice(&body);
        wire.extend_from_slice(b"\x1b[201~");

        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(&wire);

        assert_eq!(events.first(), Some(&Event::PasteStart));
        assert_eq!(events.last(), Some(&Event::PasteEnd));

        let mut assembled = Vec::new();
        for ev in &events {
            if let Event::PasteChunk(b) = ev {
                assembled.extend_from_slice(b);
            }
        }
        assert_eq!(assembled, body);
    }

    #[test]
    fn test_parse_paste_8bit_start_and_end() {
        // 8-bit C1 introducers may START the paste (0x9B = CSI, followed
        // by `200~`). The closing terminator, however, is required to be
        // the 7-bit `\x1b[201~` form: 0x9B is a valid UTF-8 continuation
        // byte and treating it as an introducer inside paste content
        // would cause real Cyrillic/CJK paste content to escape paste
        // mode (see `test_paste_body_with_0x9b_is_not_terminated`).
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x9b200~hello\x1b[201~");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], Event::PasteStart);
        assert_eq!(events[1], Event::PasteChunk(b"hello".to_vec()));
        assert_eq!(events[2], Event::PasteEnd);
    }

    #[test]
    fn test_paste_body_with_0x9b_is_not_terminated() {
        // Regression: 0x9B inside paste content (here, the second byte
        // of the UTF-8 encoding of `ћ` U+045B = D1 9B) must NOT be
        // treated as an 8-bit CSI introducer that could match a
        // following `201~` and escape paste mode. The bytes between the
        // 7-bit `\x1b[200~` start and the 7-bit `\x1b[201~` end must be
        // delivered verbatim as paste content.
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[200~hello\xd1\x9b201~world\x1b[201~");
        let chunks: Vec<&[u8]> = events
            .iter()
            .filter_map(|e| match e {
                Event::PasteChunk(b) => Some(b.as_slice()),
                _ => None,
            })
            .collect();
        let joined: Vec<u8> = chunks.concat();
        assert_eq!(joined, b"hello\xd1\x9b201~world");
        assert_eq!(events.first(), Some(&Event::PasteStart));
        assert_eq!(events.last(), Some(&Event::PasteEnd));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::PasteEnd))
                .count(),
            1,
            "exactly one PasteEnd; the 8-bit `\\x9b201~` substring must \
             not be recognised as a terminator",
        );
        // No stray keypresses (no Event::Key in the output).
        assert!(
            !events.iter().any(|e| e.as_key().is_some()),
            "paste content must not surface as keypresses: {events:?}",
        );
    }

    #[test]
    fn test_paste_body_with_0x9b_via_parse_one() {
        // Same regression as above, but exercising the `parse_one` path
        // (used by `EventSource::drain_parser`) rather than the
        // streaming `parse` path.
        let mut parser = Decoder::new(DecoderFlags::empty());
        let mut buf: Vec<u8> = Vec::from(&b"\x1b[200~hello\xd1\x9b201~world\x1b[201~"[..]);
        let mut events = Vec::new();
        loop {
            let (n, ev) = parser.parse_one(&buf);
            if n == 0 && ev.is_none() {
                break;
            }
            if n > 0 {
                buf.drain(..n);
            }
            if let Some(ev) = ev {
                events.push(ev);
            }
        }
        let chunks: Vec<u8> = events
            .iter()
            .filter_map(|e| match e {
                Event::PasteChunk(b) => Some(b.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(chunks, b"hello\xd1\x9b201~world");
        assert_eq!(events.first(), Some(&Event::PasteStart));
        assert_eq!(events.last(), Some(&Event::PasteEnd));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::PasteEnd))
                .count(),
            1,
        );
        assert!(!events.iter().any(|e| e.as_key().is_some()));
    }

    #[test]
    fn test_decoder_end_paste_escape_hatch() {
        // end_paste() force-exits paste mode when in paste, returning
        // a PasteEnd event for the caller to enqueue.
        let mut parser = Decoder::new(DecoderFlags::empty());
        let evs = parser.parse(b"\x1b[200~partial");
        assert_eq!(evs[0], Event::PasteStart);
        assert!(parser.in_paste());

        let synth = parser.end_paste();
        assert_eq!(synth, Some(Event::PasteEnd));
        assert!(!parser.in_paste());

        // Subsequent bytes are no longer treated as paste content.
        let evs = parser.parse(b"a");
        assert!(matches!(
            evs[0],
            Event::KeyPress(ref k) if k.code == KeyCode::Char('a')
        ));
    }

    #[test]
    fn test_decoder_end_paste_noop_when_not_in_paste() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        assert!(!parser.in_paste());
        assert_eq!(parser.end_paste(), None);
    }

    #[test]
    fn test_parse_c0_extras() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        // 0x1c -> Ctrl+\
        let events = parser.parse(&[0x1c]);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Char('\\'));
                assert!(k.modifiers.contains(KeyModifiers::CTRL));
            }
            _ => panic!("expected key"),
        }
    }

    #[test]
    fn test_parse_dcs_xtgettcap() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        // DCS 1+r 626f3d31 ST -> bo=1 (hex-encoded)
        let events = parser.parse(b"\x1bP1+r626F=31\x1b\\");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            Event::Termcap {
                recognized: true,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_apc_kitty_graphics() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b_Ga=T,f=32,s=2,v=2;BASE64DATA\x1b\\");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KittyGraphics { options, payload } => {
                assert_eq!(options.len(), 4);
                assert_eq!(options[0], ("a".to_string(), "T".to_string()));
                assert_eq!(payload, b"BASE64DATA");
            }
            other => panic!("expected KittyGraphics, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sos_pm() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1bXabc\x1b\\");
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::UnknownSos(_)));

        let events = parser.parse(b"\x1b^pm\x1b\\");
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::UnknownPm(_)));
    }

    #[test]
    fn test_parse_kitty_key_release() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        // CSI 97 ; 1:3 u  -> 'a' release
        let events = parser.parse(b"\x1b[97;1:3u");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyRelease(k) => assert_eq!(k.code, KeyCode::Char('a')),
            other => panic!("expected KeyRelease, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_kitty_key_with_alternates() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        // CSI 97:65:97 ; 2 u  -> 'a' with shifted='A', base='a', shift mod
        let events = parser.parse(b"\x1b[97:65:97;2u");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Char('a'));
                assert_eq!(k.shifted_key, Some('A'));
                assert_eq!(k.base_key, Some('a'));
                assert!(k.modifiers.contains(KeyModifiers::SHIFT));
            }
            other => panic!("expected Key, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_multiple_events() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"abc");
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn test_parse_ss3_f1() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1bOP");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => assert_eq!(k.code, KeyCode::F(1)),
            _ => panic!("Expected Key event"),
        }
    }

    #[test]
    fn test_parse_da1() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[?64;1;2;6;9c");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::PrimaryDeviceAttributes(p) => {
                assert_eq!(p, &vec![Some(64), Some(1), Some(2), Some(6), Some(9)])
            }
            other => panic!("Expected PrimaryDeviceAttributes, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_da2() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[>1;95;0c");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::SecondaryDeviceAttributes(p) => {
                assert_eq!(p, &vec![Some(1), Some(95), Some(0)])
            }
            other => panic!("Expected SecondaryDeviceAttributes, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_decrpm() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        // CSI ? 1049 ; 1 $ y  -> alt-screen is Set
        let events = parser.parse(b"\x1b[?1049;1$y");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::ModeReport { mode, setting } => {
                assert_eq!(*mode, crate::ansi::mode::Mode::ALT_SCREEN_SAVE_CURSOR);
                assert_eq!(*setting, crate::ansi::mode::ModeSetting::Set);
            }
            other => panic!("Expected ModeReport, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_pixel_size() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        // CSI 4 ; 600 ; 800 t
        let events = parser.parse(b"\x1b[4;600;800t");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            Event::WindowPixelSize {
                width: 800,
                height: 600
            }
        );
    }

    #[test]
    fn test_parse_cell_size() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[6;16;8t");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            Event::CellPixelSize {
                width: 8,
                height: 16
            }
        );
    }

    #[test]
    fn test_parse_kitty_keyboard_enhancements() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        // CSI ? 5 u (DISAMBIGUATE | REPORT_ALT_KEYS)
        let events = parser.parse(b"\x1b[?5u");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            Event::KittyKeyboardEnhancements(
                crate::ansi::KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
                    | crate::ansi::KittyKeyboardFlags::REPORT_ALTERNATE_KEYS
            )
        );
    }

    #[test]
    fn test_parse_osc_fg_color() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b]10;rgb:abcd/0000/ffff\x1b\\");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::ForegroundColor(Color::Rgb(r, g, b)) => {
                assert_eq!((*r, *g, *b), (0xab, 0x00, 0xff));
            }
            other => panic!("Expected ForegroundColor, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_osc_palette_color() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        // OSC 4 ; 5 ; rgb:.... reply for palette index 5.
        let events = parser.parse(b"\x1b]4;5;rgb:abcd/0000/ffff\x1b\\");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::PaletteColor {
                index,
                color: Color::Rgb(r, g, b),
            } => {
                assert_eq!(*index, 5);
                assert_eq!((*r, *g, *b), (0xab, 0x00, 0xff));
            }
            other => panic!("Expected PaletteColor, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_osc_clipboard() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b]52;c;SGVsbG8=\x07");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Clipboard { selection, content } => {
                assert_eq!(*selection, ClipboardSelection::System);
                assert_eq!(content, "SGVsbG8=");
            }
            other => panic!("Expected Clipboard, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_modify_other_keys() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[>4;2m");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::ModifyOtherKeys(m) => {
                assert_eq!(*m, crate::event::ModifyOtherKeysMode::Mode2)
            }
            other => panic!("Expected ModifyOtherKeys, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_urxvt_mouse() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        // URxvt: CSI Cb;Cx;Cy M with Cb=32 (left button press), col 11, row 21
        let events = parser.parse(b"\x1b[32;11;21M");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::MouseClick(m) => {
                assert_eq!(m.x, 10);
                assert_eq!(m.y, 20);
                assert_eq!(m.button, crate::event::MouseButton::Left);
            }
            other => panic!("Expected Mouse Click, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_utf8_mouse() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        parser.set_utf8_mouse(true);
        // 3 bytes: cb=32, cx=33, cy=33 → button Left, x=0, y=0
        let events = parser.parse(b"\x1b[M\x20\x21\x21");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::MouseClick(m) => {
                assert_eq!(m.x, 0);
                assert_eq!(m.y, 0);
            }
            other => panic!("Expected Mouse Click, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sgr_pixel_mouse_decoded_as_offsets() {
        // SGR-Pixel (mode 1016) uses the same wire format as SGR (1006). The
        // parser doesn't distinguish; it just emits 0-based offsets and the
        // caller interprets them as pixels.
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[<0;100;200M");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::MouseClick(m) => {
                assert_eq!(m.x, 99);
                assert_eq!(m.y, 199);
            }
            other => panic!("Expected Mouse Click, got {:?}", other),
        }
    }

    #[test]
    fn test_legacy_key_urxvt_shift_arrows() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[a");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Up);
                assert!(k.modifiers.contains(KeyModifiers::SHIFT));
            }
            other => panic!("Expected Key Up+Shift, got {:?}", other),
        }
    }

    #[test]
    fn test_legacy_key_urxvt_shift_f11() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[23$");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::F(11));
                assert!(k.modifiers.contains(KeyModifiers::SHIFT));
            }
            other => panic!("Expected Key F11+Shift, got {:?}", other),
        }
    }

    #[test]
    fn test_legacy_key_urxvt_ctrl_f1() {
        let mut parser = Decoder::new(DecoderFlags::empty());
        let events = parser.parse(b"\x1b[11^");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::F(1));
                assert!(k.modifiers.contains(KeyModifiers::CTRL));
            }
            other => panic!("Expected Key F1+Ctrl, got {:?}", other),
        }
    }

    // --- 8-bit C1 control codes -------------------------------------------

    #[test]
    fn test_c1_csi_arrow_equivalent_to_esc_csi() {
        // 0x9B is the 8-bit form of CSI; `0x9B A` should decode to Up arrow,
        // matching the 7-bit `ESC [ A` form.
        let mut a = Decoder::new(DecoderFlags::empty());
        let mut b = Decoder::new(DecoderFlags::empty());
        let evs_8bit = a.parse(&[0x9b, b'A']);
        let evs_7bit = b.parse(b"\x1b[A");
        assert_eq!(evs_8bit, evs_7bit);
        assert!(matches!(
            evs_8bit.first(),
            Some(Event::KeyPress(k)) if k.code == KeyCode::Up
        ));
    }

    #[test]
    fn test_c1_ss3_f1_equivalent_to_esc_o() {
        // 0x8F is the 8-bit form of SS3; `0x8F P` should decode to F1.
        let mut p = Decoder::new(DecoderFlags::empty());
        let evs = p.parse(&[0x8f, b'P']);
        assert!(matches!(
            evs.first(),
            Some(Event::KeyPress(k)) if k.code == KeyCode::F(1)
        ));
    }

    #[test]
    fn test_c1_osc_terminated_by_8bit_st() {
        // 0x9D = 8-bit OSC introducer, 0x9C = 8-bit ST terminator.
        let mut p = Decoder::new(DecoderFlags::empty());
        let mut buf = vec![0x9d];
        buf.extend_from_slice(b"10;rgb:1234/5678/9abc");
        buf.push(0x9c);
        let evs = p.parse(&buf);
        assert!(
            evs.iter().any(|e| matches!(e, Event::ForegroundColor(_))),
            "expected ForegroundColor event, got {:?}",
            evs
        );
    }

    #[test]
    fn test_c1_dcs_terminated_by_8bit_st() {
        // 0x90 = 8-bit DCS introducer; payload is an XTGETTCAP-style reply
        // with the cap name and value both hex-encoded (TN=xterm).
        let mut p = Decoder::new(DecoderFlags::empty());
        let mut buf = vec![0x90];
        buf.extend_from_slice(b"1+r544E=787465726D");
        buf.push(0x9c);
        let evs = p.parse(&buf);
        match evs.first() {
            Some(Event::Termcap { payload, .. }) => assert!(payload.starts_with("TN=")),
            other => panic!("expected Capability event, got {:?}", other),
        }
    }

    #[test]
    fn test_bel_does_not_terminate_dcs() {
        // BEL is an OSC-only terminator. Inside a DCS payload it must be
        // treated as part of the body, not as ST.
        let mut p = Decoder::new(DecoderFlags::empty());
        let evs = p.parse(b"\x1bP1$r0\x07more\x1b\\");
        // Single event: a Capability covering the whole payload (with BEL
        // embedded). The parser must NOT split it on BEL.
        assert_eq!(evs.len(), 1, "expected 1 event, got {:?}", evs);
        match &evs[0] {
            Event::Termcap { payload, .. } => {
                assert!(payload.starts_with("1$r0"));
                assert!(payload.contains('\u{07}'));
                assert!(payload.ends_with("more"));
            }
            other => panic!("expected Capability, got {:?}", other),
        }
    }

    #[test]
    fn test_bel_terminates_osc() {
        // OSC keeps its BEL-as-terminator behaviour.
        let mut p = Decoder::new(DecoderFlags::empty());
        let evs = p.parse(b"\x1b]10;rgb:1234/5678/9abc\x07");
        assert!(
            evs.iter().any(|e| matches!(e, Event::ForegroundColor(_))),
            "expected ForegroundColor, got {:?}",
            evs
        );
    }

    #[test]
    fn test_stray_c1_becomes_ctrl_alt_keypress() {
        // 0x9C (a stray ST byte) and other non-introducer C1 codes fall
        // through to the Ctrl+Alt+<code-0x40> encoding.
        let mut p = Decoder::new(DecoderFlags::empty());
        let evs = p.parse(&[0x9c]);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Char('\\'));
                assert!(k.modifiers.contains(KeyModifiers::CTRL));
                assert!(k.modifiers.contains(KeyModifiers::ALT));
            }
            other => panic!("expected Ctrl+Alt+\\ keypress, got {:?}", other),
        }
    }

    #[test]
    fn test_partial_c1_csi_is_incomplete_then_completes() {
        // A buffered 0x9B alone is incomplete (waiting for the final byte);
        // feeding the final byte should resolve it.
        let mut p = Decoder::new(DecoderFlags::empty());
        let evs1 = p.parse(&[0x9b]);
        assert!(evs1.is_empty(), "expected no events, got {:?}", evs1);
        assert!(p.has_pending(), "scanner should treat 0x9B as pending");
        let evs2 = p.parse(b"A");
        assert!(matches!(
            evs2.first(),
            Some(Event::KeyPress(k)) if k.code == KeyCode::Up
        ));
        assert!(!p.has_pending());
    }

    #[test]
    fn test_flush_partial_c1_emits_ctrl_alt_fallback() {
        // A timed-out stray 0x9B should resolve to Ctrl+Alt+[ (its C1
        // fallback) so no bytes are silently dropped.
        let mut p = Decoder::new(DecoderFlags::empty());
        let _ = p.parse(&[0x9b]);
        let evs = p.drain();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Char('['));
                assert!(k.modifiers.contains(KeyModifiers::CTRL));
                assert!(k.modifiers.contains(KeyModifiers::ALT));
            }
            other => panic!("expected Ctrl+Alt+[, got {:?}", other),
        }
    }

    #[test]
    fn test_win32_input_mode_press_release() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'A' (vk=0x41), Ctrl+Shift, key down, repeat 1.
        let evs = p.parse(b"\x1b[65;30;65;1;24;1_");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Char('a'));
                assert_eq!(k.shifted_key, Some('A'));
                assert!(k.modifiers.contains(KeyModifiers::CTRL));
                assert!(k.modifiers.contains(KeyModifiers::SHIFT));
            }
            other => panic!("expected KeyPress, got {:?}", other),
        }
        // Same key, key up, repeat 1.
        let evs = p.parse(b"\x1b[65;30;65;0;24;1_");
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], Event::KeyRelease(_)));
    }

    #[test]
    fn test_win32_input_mode_repeat_count() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'a' down, no modifiers, repeat 3.
        let evs = p.parse(b"\x1b[65;30;97;1;0;3_");
        assert_eq!(evs.len(), 3);
        for e in &evs {
            match e {
                Event::KeyPress(k) => assert_eq!(k.code, KeyCode::Char('a')),
                other => panic!("expected Key, got {:?}", other),
            }
        }
    }

    #[test]
    fn test_win32_input_mode_arrow() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // VK_LEFT (0x25), no character.
        let evs = p.parse(b"\x1b[37;75;0;1;0;1_");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::KeyPress(k) => assert_eq!(k.code, KeyCode::Left),
            other => panic!("expected Left, got {:?}", other),
        }
    }

    #[test]
    fn test_win32_input_mode_surrogate_pair() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // High surrogate of U+1F600 (😀): vk=0, ch=0xD83D.
        let evs = p.parse(b"\x1b[0;0;55357;1;0;1_");
        assert!(evs.is_empty(), "high surrogate should buffer silently");
        // Low surrogate: 0xDE00.
        let evs = p.parse(b"\x1b[0;0;56832;1;0;1_");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::KeyPress(k) => assert_eq!(k.code, KeyCode::Char('\u{1F600}')),
            other => panic!("expected combined char, got {:?}", other),
        }
    }

    #[test]
    fn test_modify_other_keys_2() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // Ctrl+a via modifyOtherKeys-2: CSI 27;5;97~
        let evs = p.parse(b"\x1b[27;5;97~");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Char('a'));
                assert!(k.modifiers.contains(KeyModifiers::CTRL));
            }
            other => panic!("expected Key, got {:?}", other),
        }
    }

    #[test]
    fn test_modify_other_keys_2_astral_codepoint() {
        // 😀 = U+1F600 = 128512 (above u16::MAX); the param must round-trip
        // through the parser without truncation.
        let mut p = Decoder::new(DecoderFlags::empty());
        let evs = p.parse(b"\x1b[27;0;128512~");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::KeyPress(k) => assert_eq!(k.code, KeyCode::Char('\u{1f600}')),
            other => panic!("expected Key('😀'), got {:?}", other),
        }
    }

    #[test]
    fn test_f3_cpr_ambiguity_emits_multi() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let evs = p.parse(b"\x1b[1;5R");
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::F(3));
                assert!(k.modifiers.contains(KeyModifiers::CTRL));
            }
            other => panic!("expected F3, got {:?}", other),
        }
        match &evs[1] {
            Event::CursorPosition(pos) if pos.x == 4 && pos.y == 0 => {}
            other => panic!("expected CursorPosition (4, 0), got {:?}", other),
        }
    }

    #[test]
    fn test_csi_8_t_emits_window_cell_size() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let evs = p.parse(b"\x1b[8;24;80t");
        assert_eq!(evs.len(), 1);
        assert!(matches!(
            evs[0],
            Event::WindowCellSize {
                width: 80,
                height: 24
            }
        ));
    }

    #[test]
    fn test_csi_48_t_emits_resize() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 48 ; rows ; cols ; ypixel ; xpixel t (mode 2048 report).
        let evs = p.parse(b"\x1b[48;24;80;480;1600t");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::Resize(ws) => {
                assert_eq!(ws.row, 24);
                assert_eq!(ws.col, 80);
                assert_eq!(ws.ypixel, 480);
                assert_eq!(ws.xpixel, 1600);
            }
            other => panic!("expected Resize, got {:?}", other),
        }
    }

    #[test]
    fn test_dcs_xtversion() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let evs = p.parse(b"\x1bP>|xterm 380\x1b\\");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::TerminalName(s) => assert_eq!(s, "xterm 380"),
            other => panic!("expected TerminalName, got {:?}", other),
        }
    }

    #[test]
    fn test_dcs_tertiary_da() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // Hex-encoded "~VTE" (7E 56 54 45).
        let evs = p.parse(b"\x1bP!|7E565445\x1b\\");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::TertiaryDeviceAttributes(s) => assert_eq!(s, "~VTE"),
            other => panic!("expected TertiaryDeviceAttributes, got {:?}", other),
        }
    }

    #[test]
    fn test_dcs_xtgettcap_hex_decodes_pairs() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 1+r TN=78746572 6D ; Co=323536
        // Hex-encoded "TN=xterm;Co=256".
        let evs = p.parse(b"\x1bP1+r544E=787465726D;436F=323536\x1b\\");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::Termcap { payload, .. } => assert_eq!(payload, "TN=xterm;Co=256"),
            other => panic!("expected Capability, got {:?}", other),
        }
    }

    #[test]
    fn test_dcs_xtgettcap_skips_invalid_pairs() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // Three entries: invalid hex name, valid pair, valid name-only.
        let evs = p.parse(b"\x1bP1+rZZ=AA;544E=78;6B62\x1b\\");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            // "TN=x" and "kb" survive; the bogus "ZZ=AA" entry is skipped.
            Event::Termcap { payload, .. } => assert_eq!(payload, "TN=x;kb"),
            other => panic!("expected Capability, got {:?}", other),
        }
    }

    #[test]
    fn test_dcs_xtgettcap_failure_is_reported() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // DCS 0 + r 524742 ST — a failure reply echoing the unsupported
        // "RGB" cap. It must surface as a Termcap with recognized=false and
        // the decoded payload, not be dropped.
        let evs = p.parse(b"\x1bP0+r524742\x1b\\");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::Termcap {
                recognized,
                payload,
            } => {
                assert!(!recognized);
                assert_eq!(payload, "RGB");
            }
            other => panic!("expected Termcap, got {:?}", other),
        }
    }

    #[test]
    fn test_dcs_xtgettcap_truecolor_success() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // DCS 1 + r 524742 ST — a successful reply for the boolean "RGB"
        // cap (name only, no value): recognized truecolor support.
        let evs = p.parse(b"\x1bP1+r524742\x1b\\");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::Termcap {
                recognized,
                payload,
            } => {
                assert!(recognized);
                assert_eq!(payload, "RGB");
            }
            other => panic!("expected Termcap, got {:?}", other),
        }
        // Same for the "Tc" boolean cap (hex 5463).
        let evs = p.parse(b"\x1bP1+r5463\x1b\\");
        match &evs[0] {
            Event::Termcap {
                recognized,
                payload,
            } => {
                assert!(recognized);
                assert_eq!(payload, "Tc");
            }
            other => panic!("expected Termcap, got {:?}", other),
        }
    }

    #[test]
    fn test_kitty_text_astral_codepoint() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'a' key with associated-text "😀" (U+1F600, > 0xFFFF).
        let evs = p.parse(b"\x1b[97;1;128512u");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::KeyPress(k) => assert_eq!(k.text.as_deref(), Some("\u{1F600}")),
            other => panic!("expected Key, got {:?}", other),
        }
    }

    #[test]
    fn test_win32_lock_modifiers_propagated() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'a' with CapsLock-on (cks=0x80).
        let evs = p.parse(b"\x1b[65;30;97;1;128;1_");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::KeyPress(k) => {
                assert!(k.modifiers.contains(KeyModifiers::CAPS_LOCK))
            }
            other => panic!("expected Key, got {:?}", other),
        }
    }

    #[test]
    fn test_kitty_lock_modifiers() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'a' with CapsLock (kitty bit 64, encoded as 65 = 64+1).
        let evs = p.parse(b"\x1b[97;65u");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::KeyPress(k) => {
                assert!(k.modifiers.contains(KeyModifiers::CAPS_LOCK))
            }
            other => panic!("expected Key, got {:?}", other),
        }
    }

    // --- parse_one (slice-driven) -----------------------------------------

    #[test]
    fn test_parse_one_simple_key() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let (n, ev) = p.parse_one(b"a");
        assert_eq!(n, 1);
        match ev {
            Some(Event::KeyPress(k)) => assert_eq!(k.code, KeyCode::Char('a')),
            other => panic!("expected Key('a'), got {:?}", other),
        }
    }

    #[test]
    fn test_parse_one_incomplete_returns_zero() {
        let mut p = Decoder::new(DecoderFlags::empty());
        assert_eq!(p.parse_one(b"\x1b"), (0, None));
        assert_eq!(p.parse_one(b"\x1b["), (0, None));
        assert_eq!(p.parse_one(b"\x1b[2"), (0, None));
    }

    #[test]
    fn test_parse_one_expire_leading_esc() {
        let p = Decoder::new(DecoderFlags::empty());
        match p.expire_leading(0x1b) {
            Some(Event::KeyPress(k)) => assert_eq!(k.code, KeyCode::Escape),
            other => panic!("expected Escape, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_one_paste_stream() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // PasteStart
        let (n, ev) = p.parse_one(b"\x1b[200~");
        assert_eq!(n, 6);
        assert!(matches!(ev, Some(Event::PasteStart)));

        // Chunk 1: "hello"
        let (n, ev) = p.parse_one(b"hello");
        assert_eq!(n, 5);
        match ev {
            Some(Event::PasteChunk(b)) => assert_eq!(b, b"hello"),
            other => panic!("expected PasteChunk, got {:?}", other),
        }

        // Chunk 2: " world" followed by terminator
        let (n, ev) = p.parse_one(b" world\x1b[201~");
        // Emits the content chunk first; PasteEnd is queued.
        match ev {
            Some(Event::PasteChunk(b)) => assert_eq!(b, b" world"),
            other => panic!("expected PasteChunk, got {:?}", other),
        }
        assert_eq!(n, 12);

        // Drain the queued PasteEnd.
        let (n, ev) = p.parse_one(b"");
        assert_eq!(n, 0);
        assert!(matches!(ev, Some(Event::PasteEnd)));
    }

    #[test]
    fn test_parse_one_paste_partial_terminator_holdback() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let (_, _) = p.parse_one(b"\x1b[200~"); // PasteStart
        // Buffer ends with the first two bytes of the terminator — the
        // parser must hold them back so it can complete the match on the
        // next call.
        let (n, ev) = p.parse_one(b"abc\x1b[");
        assert_eq!(n, 3);
        match ev {
            Some(Event::PasteChunk(b)) => assert_eq!(b, b"abc"),
            other => panic!("expected PasteChunk(\"abc\"), got {:?}", other),
        }
    }

    // --- chunked-input tests --------------------------------------------
    //
    // The decoder is fed in arbitrary fragments by callers (EventSource).
    // These tests verify that splitting any byte sequence at any boundary
    // produces the same Events as feeding it whole.

    #[test]
    fn test_chunked_csi_focus_then_text() {
        // First chunk is just the CSI introducer; no event yet.
        let mut p = Decoder::new(DecoderFlags::empty());
        assert_eq!(p.parse(b"\x1b["), vec![]);

        // Second chunk completes the focus-in and adds three plain chars.
        let events = p.parse(b"Iabc");
        assert_eq!(events.len(), 4);
        assert_eq!(events[0], Event::FocusIn);
        for (i, ch) in ['a', 'b', 'c'].iter().enumerate() {
            match &events[i + 1] {
                Event::KeyPress(k) => assert_eq!(k.code, KeyCode::Char(*ch)),
                other => panic!("expected Char({ch}), got {other:?}"),
            }
        }
    }

    /// Feed `data` to a fresh decoder one byte at a time, returning the
    /// collected Events. Mirrors how an `EventSource` would deliver bytes
    /// from a slow tty (one read at a time).
    fn feed_byte_by_byte(data: &[u8]) -> Vec<Event> {
        let mut p = Decoder::new(DecoderFlags::empty());
        let mut events = Vec::new();
        for b in data {
            events.extend(p.parse(std::slice::from_ref(b)));
        }
        events
    }

    #[test]
    fn test_chunked_byte_by_byte_focus_in() {
        assert_eq!(feed_byte_by_byte(b"\x1b[I"), vec![Event::FocusIn]);
    }

    #[test]
    fn test_chunked_byte_by_byte_focus_out() {
        assert_eq!(feed_byte_by_byte(b"\x1b[O"), vec![Event::FocusOut]);
    }

    #[test]
    fn test_chunked_byte_by_byte_modified_arrow() {
        // Ctrl+Up encoded as CSI 1 ; 5 A — every byte after the first
        // ESC must be buffered until A arrives.
        let events = feed_byte_by_byte(b"\x1b[1;5A");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Up);
                assert!(k.modifiers.contains(KeyModifiers::CTRL));
            }
            other => panic!("expected Ctrl+Up, got {other:?}"),
        }
    }

    #[test]
    fn test_chunked_byte_by_byte_sgr_mouse_press() {
        // SGR mouse press button 0 at (10,20).
        let events = feed_byte_by_byte(b"\x1b[<0;11;21M");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::MouseClick(m) => {
                assert_eq!(m.x, 10);
                assert_eq!(m.y, 20);
                assert_eq!(m.button, crate::event::MouseButton::Left);
            }
            other => panic!("expected Mouse Click, got {other:?}"),
        }
    }

    #[test]
    fn test_chunked_byte_by_byte_utf8_multibyte_char() {
        // 📺 is U+1F4FA, encoded as four UTF-8 bytes (F0 9F 93 BA).
        // Feeding one byte at a time must coalesce them into a single
        // Char keypress without emitting Unknown for partial bytes.
        let events = feed_byte_by_byte("📺".as_bytes());
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => assert_eq!(k.code, KeyCode::Char('📺')),
            other => panic!("expected Char('📺'), got {other:?}"),
        }
    }

    #[test]
    fn test_chunked_byte_by_byte_bracketed_paste() {
        // PasteStart, content "hi📺", PasteEnd — fed one byte at a time.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x1b[200~hi");
        bytes.extend_from_slice("📺".as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");

        let events = feed_byte_by_byte(&bytes);
        // PasteStart first, PasteEnd last, with any number of PasteChunk
        // events in between whose concatenation matches the original
        // content exactly, regardless of UTF-8 byte boundaries.
        assert_eq!(events.first(), Some(&Event::PasteStart));
        assert_eq!(events.last(), Some(&Event::PasteEnd));
        let mut chunk_count = 0;
        let mut buf = Vec::new();
        for ev in &events {
            if let Event::PasteChunk(b) = ev {
                chunk_count += 1;
                buf.extend_from_slice(b);
            }
        }
        assert!(chunk_count >= 1, "expected at least one PasteChunk");
        // The bytes between PasteStart and PasteEnd should round-trip
        // verbatim when reassembled by the caller.
        assert_eq!(buf, "hi📺".as_bytes());
    }

    #[test]
    fn test_chunked_split_in_middle_of_csi_params() {
        // Ctrl+Up split in the middle of the parameter list.
        let mut p = Decoder::new(DecoderFlags::empty());
        assert_eq!(p.parse(b"\x1b[1;"), vec![]);
        let events = p.parse(b"5A");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Up);
                assert!(k.modifiers.contains(KeyModifiers::CTRL));
            }
            other => panic!("expected Ctrl+Up, got {other:?}"),
        }
    }

    #[test]
    fn test_chunked_split_after_intermediate_only() {
        // Just the ESC by itself should buffer with no event yet.
        let mut p = Decoder::new(DecoderFlags::empty());
        assert_eq!(p.parse(b"\x1b"), vec![]);
        assert_eq!(p.parse(b"["), vec![]);
        assert_eq!(p.parse(b"I"), vec![Event::FocusIn]);
    }

    fn press(events: Vec<Event>) -> Key {
        match events.as_slice() {
            [Event::KeyPress(k)] => k.clone(),
            other => panic!("expected single KeyPress, got {other:?}"),
        }
    }

    #[test]
    fn decoder_flag_ctrl_i_swaps_tab() {
        let mut p = Decoder::new(DecoderFlags::empty());
        assert_eq!(press(p.parse(b"\t")).code, KeyCode::Tab);

        let mut p = Decoder::new(DecoderFlags::CTRL_I);
        let k = press(p.parse(b"\t"));
        assert_eq!(k.code, KeyCode::Char('i'));
        assert_eq!(k.modifiers, KeyModifiers::CTRL);
    }

    #[test]
    fn decoder_flag_ctrl_m_swaps_enter() {
        let mut p = Decoder::new(DecoderFlags::empty());
        assert_eq!(press(p.parse(b"\r")).code, KeyCode::Enter);

        let mut p = Decoder::new(DecoderFlags::CTRL_M);
        let k = press(p.parse(b"\r"));
        assert_eq!(k.code, KeyCode::Char('m'));
        assert_eq!(k.modifiers, KeyModifiers::CTRL);
    }

    #[test]
    fn decoder_flag_ctrl_at_swaps_ctrl_space() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let k = press(p.parse(b"\x00"));
        assert_eq!(k.code, KeyCode::Space);
        assert_eq!(k.modifiers, KeyModifiers::CTRL);

        let mut p = Decoder::new(DecoderFlags::CTRL_AT);
        let k = press(p.parse(b"\x00"));
        assert_eq!(k.code, KeyCode::Char('@'));
        assert_eq!(k.modifiers, KeyModifiers::CTRL);
    }

    #[test]
    fn decoder_flag_backspace_is_delete() {
        let mut p = Decoder::new(DecoderFlags::empty());
        assert_eq!(press(p.parse(b"\x7f")).code, KeyCode::Backspace);

        let mut p = Decoder::new(DecoderFlags::BACKSPACE_IS_DELETE);
        assert_eq!(press(p.parse(b"\x7f")).code, KeyCode::Delete);
    }

    #[test]
    fn decoder_flag_ctrl_open_bracket_swaps_lone_esc() {
        // Default: lone ESC -> Escape.
        let p = Decoder::new(DecoderFlags::empty());
        let ev = p.expire_leading(0x1b).expect("expire returns event");
        match ev {
            Event::KeyPress(k) => assert_eq!(k.code, KeyCode::Escape),
            other => panic!("expected KeyPress(Escape), got {other:?}"),
        }

        // With CTRL_OPEN_BRACKET: lone ESC -> Ctrl+[.
        let p = Decoder::new(DecoderFlags::CTRL_OPEN_BRACKET);
        let ev = p.expire_leading(0x1b).expect("expire returns event");
        match ev {
            Event::KeyPress(k) => {
                assert_eq!(k.code, KeyCode::Char('['));
                assert_eq!(k.modifiers, KeyModifiers::CTRL);
            }
            other => panic!("expected Ctrl+[, got {other:?}"),
        }
    }

    #[test]
    fn decoder_flag_find_key_swaps_csi_1_tilde() {
        let mut p = Decoder::new(DecoderFlags::empty());
        assert_eq!(press(p.parse(b"\x1b[1~")).code, KeyCode::Home);

        let mut p = Decoder::new(DecoderFlags::FIND_KEY);
        assert_eq!(press(p.parse(b"\x1b[1~")).code, KeyCode::Find);

        // Code 7 (Home alias) is unaffected.
        let mut p = Decoder::new(DecoderFlags::FIND_KEY);
        assert_eq!(press(p.parse(b"\x1b[7~")).code, KeyCode::Home);
    }

    #[test]
    fn decoder_flag_select_key_swaps_csi_4_tilde() {
        let mut p = Decoder::new(DecoderFlags::empty());
        assert_eq!(press(p.parse(b"\x1b[4~")).code, KeyCode::End);

        let mut p = Decoder::new(DecoderFlags::SELECT_KEY);
        assert_eq!(press(p.parse(b"\x1b[4~")).code, KeyCode::Select);

        // Code 8 (End alias) is unaffected.
        let mut p = Decoder::new(DecoderFlags::SELECT_KEY);
        assert_eq!(press(p.parse(b"\x1b[8~")).code, KeyCode::End);
    }

    #[test]
    fn decoder_flag_find_key_swaps_urxvt_modifier_suffix() {
        // URxvt: ESC [ 1 $ -> Shift+Home; with FIND_KEY -> Shift+Find.
        let mut p = Decoder::new(DecoderFlags::FIND_KEY);
        let k = press(p.parse(b"\x1b[1$"));
        assert_eq!(k.code, KeyCode::Find);
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn esc_esc_alone_waits_for_timeout_then_alt_esc() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // Before expiry, holding ESC ESC produces no event.
        assert_eq!(p.parse(b"\x1b\x1b"), vec![]);
        // After drain (timeout) the pair resolves to Alt+Esc.
        let events = p.drain();
        match events.as_slice() {
            [Event::KeyPress(k)] => {
                assert_eq!(k.code, KeyCode::Escape);
                assert_eq!(k.modifiers, KeyModifiers::ALT);
            }
            other => panic!("expected single Alt+Esc, got {other:?}"),
        }
    }

    #[test]
    fn esc_esc_arrow_promotes_to_alt_up() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let k = press(p.parse(b"\x1b\x1b[A"));
        assert_eq!(k.code, KeyCode::Up);
        assert_eq!(k.modifiers, KeyModifiers::ALT);
    }

    #[test]
    fn esc_esc_printable_yields_lone_esc_then_alt_char() {
        // ESC ESC <printable>: the inner ESC <printable> already
        // resolves to Alt+<printable>, so the outer ESC has nothing
        // new to add and lands as a standalone Esc keypress first.
        let mut p = Decoder::new(DecoderFlags::empty());
        let events = p.parse(b"\x1b\x1ba");
        match events.as_slice() {
            [Event::KeyPress(esc), Event::KeyPress(alt_a)] => {
                assert_eq!(esc.code, KeyCode::Escape);
                assert!(!esc.modifiers.contains(KeyModifiers::ALT));
                assert_eq!(alt_a.code, KeyCode::Char('a'));
                assert_eq!(alt_a.modifiers, KeyModifiers::ALT);
            }
            other => panic!("expected [Esc, Alt+a], got {other:?}"),
        }
    }

    #[test]
    fn esc_esc_modified_arrow_compounds_alt() {
        // Shift+Up via CSI 1;2A, prefixed with another ESC -> Shift+Alt+Up.
        let mut p = Decoder::new(DecoderFlags::empty());
        let k = press(p.parse(b"\x1b\x1b[1;2A"));
        assert_eq!(k.code, KeyCode::Up);
        assert!(k.modifiers.contains(KeyModifiers::ALT));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn esc_esc_non_key_emits_lone_esc_then_inner() {
        // ESC followed by a focus-in (CSI I, non-key). The leading ESC has
        // nothing to promote, so it lands as a standalone Esc keypress and
        // FocusIn follows on the next iteration.
        let mut p = Decoder::new(DecoderFlags::empty());
        let events = p.parse(b"\x1b\x1b[I");
        match events.as_slice() {
            [Event::KeyPress(k), Event::FocusIn] => {
                assert_eq!(k.code, KeyCode::Escape);
                assert!(!k.modifiers.contains(KeyModifiers::ALT));
            }
            other => panic!("expected [Esc, FocusIn], got {other:?}"),
        }
    }

    #[test]
    fn triple_esc_then_arrow_yields_lone_esc_then_alt_up() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let events = p.parse(b"\x1b\x1b\x1b[A");
        match events.as_slice() {
            [Event::KeyPress(esc), Event::KeyPress(alt_up)] => {
                assert_eq!(esc.code, KeyCode::Escape);
                assert!(!esc.modifiers.contains(KeyModifiers::ALT));
                assert_eq!(alt_up.code, KeyCode::Up);
                assert_eq!(alt_up.modifiers, KeyModifiers::ALT);
            }
            other => panic!("expected [Esc, Alt+Up], got {other:?}"),
        }
    }

    #[test]
    fn triple_esc_alone_drains_to_lone_esc_then_alt_esc() {
        let mut p = Decoder::new(DecoderFlags::empty());
        assert_eq!(p.parse(b"\x1b\x1b\x1b"), vec![]);
        let events = p.drain();
        match events.as_slice() {
            [Event::KeyPress(esc), Event::KeyPress(alt_esc)] => {
                assert_eq!(esc.code, KeyCode::Escape);
                assert!(!esc.modifiers.contains(KeyModifiers::ALT));
                assert_eq!(alt_esc.code, KeyCode::Escape);
                assert_eq!(alt_esc.modifiers, KeyModifiers::ALT);
            }
            other => panic!("expected [Esc, Alt+Esc], got {other:?}"),
        }
    }

    #[test]
    fn esc_alt_letter_unchanged() {
        // Regression: a single ESC + printable still resolves to Alt+<char>
        // with no extra events.
        let mut p = Decoder::new(DecoderFlags::empty());
        let k = press(p.parse(b"\x1ba"));
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.modifiers, KeyModifiers::ALT);
    }

    #[test]
    fn esc_run_drains_letters_and_holds_trailing_esc() {
        // `\x1babc\x1b` (no timeout) drains Alt+a, b, c and leaves the
        // trailing ESC pending for the next event or timeout.
        let mut p = Decoder::new(DecoderFlags::empty());
        let events = p.parse(b"\x1babc\x1b");
        match events.as_slice() {
            [
                Event::KeyPress(alt_a),
                Event::KeyPress(b),
                Event::KeyPress(c),
            ] => {
                assert_eq!(alt_a.code, KeyCode::Char('a'));
                assert_eq!(alt_a.modifiers, KeyModifiers::ALT);
                assert_eq!(b.code, KeyCode::Char('b'));
                assert!(b.modifiers.is_empty());
                assert_eq!(c.code, KeyCode::Char('c'));
                assert!(c.modifiers.is_empty());
            }
            other => panic!("expected [Alt+a, b, c], got {other:?}"),
        }
        // The trailing ESC resolves on timeout.
        let tail = p.drain();
        match tail.as_slice() {
            [Event::KeyPress(esc)] => {
                assert_eq!(esc.code, KeyCode::Escape);
                assert!(esc.modifiers.is_empty());
            }
            other => panic!("expected trailing Esc, got {other:?}"),
        }
    }

    /// Kitty keyboard event-type sub-parameter (`params[1]:1`) must
    /// be honoured for non-CSI-u keys (cursor keys, function keys
    /// CSI P/Q/S, navigation keys CSI ~). Without it, key release and
    /// repeat collapse to ordinary press, leading to duplicate
    /// shortcut handling on the host application.
    #[test]
    fn kitty_event_type_release_for_csi_tilde_navigation_key() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 6;6:3~  =  PageDown, mods=Ctrl+Shift, event-type=release
        let evs = p.parse(b"\x1b[6;6:3~");
        match evs.as_slice() {
            [Event::KeyRelease(k)] => {
                assert_eq!(k.code, KeyCode::PageDown);
                assert_eq!(k.modifiers, KeyModifiers::CTRL | KeyModifiers::SHIFT);
            }
            other => panic!("expected single KeyRelease, got {other:?}"),
        }
    }

    #[test]
    fn kitty_event_type_repeat_for_csi_tilde_navigation_key() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 6;6:2~  =  PageDown, mods=Ctrl+Shift, event-type=repeat
        let evs = p.parse(b"\x1b[6;6:2~");
        match evs.as_slice() {
            [Event::KeyRepeat(k)] => {
                assert_eq!(k.code, KeyCode::PageDown);
                assert_eq!(k.modifiers, KeyModifiers::CTRL | KeyModifiers::SHIFT);
            }
            other => panic!("expected single KeyRepeat, got {other:?}"),
        }
    }

    #[test]
    fn kitty_event_type_release_for_csi_cursor_key() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 1;6:3 A  =  Up, mods=Ctrl+Shift, event-type=release
        let evs = p.parse(b"\x1b[1;6:3A");
        match evs.as_slice() {
            [Event::KeyRelease(k)] => {
                assert_eq!(k.code, KeyCode::Up);
                assert_eq!(k.modifiers, KeyModifiers::CTRL | KeyModifiers::SHIFT);
            }
            other => panic!("expected single KeyRelease, got {other:?}"),
        }
    }

    #[test]
    fn kitty_event_type_repeat_for_csi_cursor_key() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 1;6:2 A  =  Up, mods=Ctrl+Shift, event-type=repeat
        let evs = p.parse(b"\x1b[1;6:2A");
        match evs.as_slice() {
            [Event::KeyRepeat(k)] => {
                assert_eq!(k.code, KeyCode::Up);
                assert_eq!(k.modifiers, KeyModifiers::CTRL | KeyModifiers::SHIFT);
            }
            other => panic!("expected single KeyRepeat, got {other:?}"),
        }
    }

    #[test]
    fn kitty_event_type_release_for_csi_function_key() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 1;3:3 P  =  F1, mods=Alt, event-type=release
        let evs = p.parse(b"\x1b[1;3:3P");
        match evs.as_slice() {
            [Event::KeyRelease(k)] => {
                assert_eq!(k.code, KeyCode::F(1));
                assert_eq!(k.modifiers, KeyModifiers::ALT);
            }
            other => panic!("expected single KeyRelease, got {other:?}"),
        }
    }

    #[test]
    fn kitty_event_type_release_for_csi_z_shift_tab() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 1;2:3 Z  =  Shift+Tab, event-type=release
        let evs = p.parse(b"\x1b[1;2:3Z");
        match evs.as_slice() {
            [Event::KeyRelease(k)] => {
                assert_eq!(k.code, KeyCode::Tab);
                assert_eq!(k.modifiers, KeyModifiers::SHIFT);
            }
            other => panic!("expected single KeyRelease, got {other:?}"),
        }
    }

    /// Default phase (no event-type sub-param): keys still decode as
    /// press. Guards against the helper accidentally treating "absent"
    /// as "release".
    #[test]
    fn kitty_event_type_absent_defaults_to_press() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 6;6~  =  PageDown, mods=Ctrl+Shift, no event-type
        let k = press(p.parse(b"\x1b[6;6~"));
        assert_eq!(k.code, KeyCode::PageDown);
        assert_eq!(k.modifiers, KeyModifiers::CTRL | KeyModifiers::SHIFT);
    }

    // ---------------------------------------------------------------
    // Shifted-input coverage across decoder paths and scripts.
    //
    // Each block exercises one entry point with the same set of
    // codepoints so regressions in case-folding, shifted_key
    // synthesis, and text auto-population surface uniformly.
    // ---------------------------------------------------------------

    // --- Bare-byte path (ASCII) ------------------------------------

    #[test]
    fn bare_ascii_uppercase_letter_canonicalizes() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let k = press(p.parse(b"A"));
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn bare_ascii_lowercase_letter_unshifted() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let k = press(p.parse(b"a"));
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        assert_eq!(k.text.as_deref(), Some("a"));
    }

    #[test]
    fn bare_ascii_digit_no_case_variant() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let k = press(p.parse(b"1"));
        assert_eq!(k.code, KeyCode::Char('1'));
        assert_eq!(k.shifted_key, None);
        assert_eq!(k.text.as_deref(), Some("1"));
    }

    // --- UTF-8 path ------------------------------------------------

    #[test]
    fn utf8_cyrillic_uppercase_canonicalizes() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // Ц = U+0426 = 0xD0 0xA6
        let k = press(p.parse("Ц".as_bytes()));
        assert_eq!(k.code, KeyCode::Char('ц'));
        assert_eq!(k.shifted_key, Some('Ц'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Ц"));
    }

    #[test]
    fn utf8_cyrillic_lowercase_unshifted() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let k = press(p.parse("ц".as_bytes()));
        assert_eq!(k.code, KeyCode::Char('ц'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        assert_eq!(k.text.as_deref(), Some("ц"));
    }

    #[test]
    fn utf8_greek_uppercase_canonicalizes() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let k = press(p.parse("Α".as_bytes()));
        assert_eq!(k.code, KeyCode::Char('α'));
        assert_eq!(k.shifted_key, Some('Α'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Α"));
    }

    #[test]
    fn utf8_german_eszett_no_uppercase_single_cp() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'ß' uppercases to "SS" (multi-cp) — helper leaves it alone,
        // but text auto-populates with the original codepoint.
        let k = press(p.parse("ß".as_bytes()));
        assert_eq!(k.code, KeyCode::Char('ß'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        assert_eq!(k.text.as_deref(), Some("ß"));
    }

    #[test]
    fn utf8_turkish_i_with_dot_multi_codepoint_lower() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'İ' lowercases to "i\u{307}" (multi-cp); helper bails.
        let k = press(p.parse("İ".as_bytes()));
        assert_eq!(k.code, KeyCode::Char('İ'));
        assert_eq!(k.shifted_key, None);
        assert_eq!(k.text.as_deref(), Some("İ"));
    }

    #[test]
    fn utf8_arabic_no_case_variant() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // Arabic letter alef — no case folding.
        let k = press(p.parse("ا".as_bytes()));
        assert_eq!(k.code, KeyCode::Char('ا'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        assert_eq!(k.text.as_deref(), Some("ا"));
    }

    #[test]
    fn utf8_hebrew_no_case_variant() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // Hebrew letter alef — no case folding.
        let k = press(p.parse("א".as_bytes()));
        assert_eq!(k.code, KeyCode::Char('א'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.is_empty());
        assert_eq!(k.text.as_deref(), Some("א"));
    }

    // --- Kitty CSI u path -----------------------------------------

    #[test]
    fn kitty_csi_u_cyrillic_uppercase() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'Ц' = 1062. Bare kitty CSI u with no shifted/base reported.
        let k = press(p.parse(b"\x1b[1062u"));
        assert_eq!(k.code, KeyCode::Char('ц'));
        assert_eq!(k.shifted_key, Some('Ц'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Ц"));
    }

    #[test]
    fn kitty_csi_u_cyrillic_lowercase_with_shift() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'ц' = 1094, modifiers=Shift (mod value 2 = 1<<0 + 1).
        let k = press(p.parse(b"\x1b[1094;2u"));
        assert_eq!(k.code, KeyCode::Char('ц'));
        assert_eq!(k.shifted_key, Some('Ц'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Ц"));
    }

    #[test]
    fn kitty_csi_u_greek_uppercase() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'Α' = 913
        let k = press(p.parse(b"\x1b[913u"));
        assert_eq!(k.code, KeyCode::Char('α'));
        assert_eq!(k.shifted_key, Some('Α'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Α"));
    }

    #[test]
    fn kitty_csi_u_turkish_multi_cp_lower_passthrough() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'İ' = 304, lowercases to "i\u{307}" — helper leaves it alone.
        let k = press(p.parse(b"\x1b[304u"));
        assert_eq!(k.code, KeyCode::Char('İ'));
        assert_eq!(k.shifted_key, None);
        assert_eq!(k.text.as_deref(), Some("İ"));
    }

    #[test]
    fn kitty_csi_u_arabic_with_shift_no_synthesis() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'ا' = 1575, modifiers=Shift. No case variant; shifted_key
        // stays None; text still surfaces the typed glyph.
        let k = press(p.parse(b"\x1b[1575;2u"));
        assert_eq!(k.code, KeyCode::Char('ا'));
        assert_eq!(k.shifted_key, None);
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("ا"));
    }

    // --- modifyOtherKeys-2 path -----------------------------------

    #[test]
    fn mok2_cyrillic_uppercase_letter() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 27 ; 1 ; 1062 ~  (no modifier, codepoint 'Ц')
        let k = press(p.parse(b"\x1b[27;1;1062~"));
        assert_eq!(k.code, KeyCode::Char('ц'));
        assert_eq!(k.shifted_key, Some('Ц'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Ц"));
    }

    #[test]
    fn mok2_greek_lowercase_with_shift() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 27 ; 2 ; 945 ~  (Shift, codepoint 'α'); '2' encodes Shift.
        let k = press(p.parse(b"\x1b[27;2;945~"));
        assert_eq!(k.code, KeyCode::Char('α'));
        assert_eq!(k.shifted_key, Some('Α'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Α"));
    }

    #[test]
    fn mok2_ctrl_cyrillic_suppresses_text() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 27 ; 5 ; 1094 ~  (Ctrl, codepoint 'ц')
        let k = press(p.parse(b"\x1b[27;5;1094~"));
        assert_eq!(k.code, KeyCode::Char('ц'));
        assert!(k.modifiers.contains(KeyModifiers::CTRL));
        assert!(k.text.is_none());
    }

    // --- win32 input mode -----------------------------------------

    #[test]
    fn win32_uppercase_letter_with_shift_only() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'A' (vk=0x41=65), Shift (cks=0x10=16), key down, repeat 1.
        let evs = p.parse(b"\x1b[65;30;65;1;16;1_");
        let Event::KeyPress(k) = evs.into_iter().next().unwrap() else {
            panic!()
        };
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn win32_uppercase_letter_via_caps_lock_no_synthetic_shift() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'A' produced by CapsLock alone (cks=0x80=128), key down.
        let evs = p.parse(b"\x1b[65;30;65;1;128;1_");
        let Event::KeyPress(k) = evs.into_iter().next().unwrap() else {
            panic!()
        };
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::CAPS_LOCK));
        assert!(
            !k.modifiers.contains(KeyModifiers::SHIFT),
            "CapsLock alone must not synthesize SHIFT"
        );
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn win32_lowercase_letter_via_shift_plus_caps_lock() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'a' produced by Shift+CapsLock (cks=0x10|0x80=144), key down.
        let evs = p.parse(b"\x1b[65;30;97;1;144;1_");
        let Event::KeyPress(k) = evs.into_iter().next().unwrap() else {
            panic!()
        };
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert!(k.modifiers.contains(KeyModifiers::CAPS_LOCK));
        assert_eq!(k.text.as_deref(), Some("a"));
    }

    #[test]
    fn win32_cyrillic_uppercase_letter_with_shift() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // 'Ц' = U+0426 = 1062, vk arbitrary, cks Shift only.
        let evs = p.parse(b"\x1b[65;30;1062;1;16;1_");
        let Event::KeyPress(k) = evs.into_iter().next().unwrap() else {
            panic!()
        };
        assert_eq!(k.code, KeyCode::Char('ц'));
        assert_eq!(k.shifted_key, Some('Ц'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("Ц"));
    }

    // --- Legacy fixterms CSI u shape -----------------------------------
    //
    // Fixterms predates the kitty extension and uses bare
    // `CSI codepoint ; mods u` with no shifted/base subparams. The
    // modifier value follows the same 1-based scheme kitty uses, so
    // these sequences round-trip through the kitty decoder unchanged.

    #[test]
    fn fixterms_bare_uppercase_letter_canonicalizes() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let k = press(p.parse(b"\x1b[65u"));
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn fixterms_shift_lowercase_letter_synthesizes_shifted() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 97;2 u = 'a' + Shift.
        let k = press(p.parse(b"\x1b[97;2u"));
        assert_eq!(k.code, KeyCode::Char('a'));
        assert_eq!(k.shifted_key, Some('A'));
        assert!(k.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(k.text.as_deref(), Some("A"));
    }

    #[test]
    fn fixterms_ctrl_letter_suppresses_text() {
        let mut p = Decoder::new(DecoderFlags::empty());
        // CSI 97;5 u = Ctrl+a.
        let k = press(p.parse(b"\x1b[97;5u"));
        assert_eq!(k.code, KeyCode::Char('a'));
        assert!(k.modifiers.contains(KeyModifiers::CTRL));
        assert!(k.text.is_none());
        assert_eq!(k.shifted_key, None);
    }

    // --- Cross-decoder identity ----------------------------------------
    //
    // For a given logical press, every decoder path must produce a Key
    // whose (code, modifiers) match — that pair is the binding identity
    // used by `==` and `Hash`. Informational fields (`text`,
    // `shifted_key`, `base_key`) may legitimately differ depending on
    // which kitty enhancement flags the terminal advertises.

    fn first_press(p: &mut Decoder, bytes: &[u8]) -> Key {
        let evs = p.parse(bytes);
        let Event::KeyPress(k) = evs.into_iter().next().unwrap() else {
            panic!("expected KeyPress, got something else")
        };
        k
    }

    #[test]
    fn cross_decoder_plain_a_matches() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let bare = first_press(&mut p, b"a");
        let mut p = Decoder::new(DecoderFlags::empty());
        let kitty_bare = first_press(&mut p, b"\x1b[97u");
        let mut p = Decoder::new(DecoderFlags::empty());
        // Kitty with associated text (no shifted/base).
        let kitty_text = first_press(&mut p, b"\x1b[97;1;97u");
        let mut p = Decoder::new(DecoderFlags::empty());
        // Kitty with alternate keys.
        let kitty_alt = first_press(&mut p, b"\x1b[97:97:97u");
        let mut p = Decoder::new(DecoderFlags::empty());
        let mok2 = first_press(&mut p, b"\x1b[27;1;97~");
        assert_eq!(bare, kitty_bare);
        assert_eq!(bare, kitty_text);
        assert_eq!(bare, kitty_alt);
        assert_eq!(bare, mok2);
    }

    #[test]
    fn cross_decoder_shift_a_matches() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let bare = first_press(&mut p, b"A");
        let mut p = Decoder::new(DecoderFlags::empty());
        let kitty_bare = first_press(&mut p, b"\x1b[65u");
        let mut p = Decoder::new(DecoderFlags::empty());
        // Kitty all-keys: codepoint stays unshifted (97), modifier=2 (Shift).
        let kitty_all = first_press(&mut p, b"\x1b[97;2u");
        let mut p = Decoder::new(DecoderFlags::empty());
        // Kitty alt-keys: code:shifted:base.
        let kitty_alt = first_press(&mut p, b"\x1b[97:65:97;2u");
        let mut p = Decoder::new(DecoderFlags::empty());
        // Kitty assoc-text: text codepoint 65 ('A').
        let kitty_text = first_press(&mut p, b"\x1b[97;2;65u");
        let mut p = Decoder::new(DecoderFlags::empty());
        let mok2 = first_press(&mut p, b"\x1b[27;2;97~");
        assert_eq!(bare, kitty_bare);
        assert_eq!(bare, kitty_all);
        assert_eq!(bare, kitty_alt);
        assert_eq!(bare, kitty_text);
        assert_eq!(bare, mok2);
    }

    #[test]
    fn cross_decoder_ctrl_a_matches() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let bare = first_press(&mut p, b"\x01");
        let mut p = Decoder::new(DecoderFlags::empty());
        let kitty = first_press(&mut p, b"\x1b[97;5u");
        let mut p = Decoder::new(DecoderFlags::empty());
        let mok2 = first_press(&mut p, b"\x1b[27;5;97~");
        assert_eq!(bare, kitty);
        assert_eq!(bare, mok2);
    }

    #[test]
    fn cross_decoder_alt_a_matches() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let bare = first_press(&mut p, b"\x1ba");
        let mut p = Decoder::new(DecoderFlags::empty());
        let kitty = first_press(&mut p, b"\x1b[97;3u");
        let mut p = Decoder::new(DecoderFlags::empty());
        let mok2 = first_press(&mut p, b"\x1b[27;3;97~");
        assert_eq!(bare, kitty);
        assert_eq!(bare, mok2);
    }

    #[test]
    fn cross_decoder_cyrillic_upper_matches() {
        // 'Ц' = U+0426 = 0xD0 0xA6.
        let mut p = Decoder::new(DecoderFlags::empty());
        let utf8 = first_press(&mut p, "Ц".as_bytes());
        let mut p = Decoder::new(DecoderFlags::empty());
        // Kitty bare codepoint for Ц.
        let kitty_bare = first_press(&mut p, b"\x1b[1062u");
        let mut p = Decoder::new(DecoderFlags::empty());
        // Kitty all-keys for Shift+ц: code stays lowercase, mod=2.
        let kitty_all = first_press(&mut p, b"\x1b[1094;2u");
        let mut p = Decoder::new(DecoderFlags::empty());
        // Kitty alt-keys: code:shifted (Ц).
        let kitty_alt = first_press(&mut p, b"\x1b[1094:1062;2u");
        assert_eq!(utf8, kitty_bare);
        assert_eq!(utf8, kitty_all);
        assert_eq!(utf8, kitty_alt);
    }

    #[test]
    fn cross_decoder_shift_tab_matches() {
        // Bare CSI Z, kitty, and MOK2 all decode Shift+Tab to the
        // same canonical form (Tab + SHIFT).
        let mut p = Decoder::new(DecoderFlags::empty());
        let bare = first_press(&mut p, b"\x1b[Z");
        let mut p = Decoder::new(DecoderFlags::empty());
        let kitty = first_press(&mut p, b"\x1b[9;2u");
        let mut p = Decoder::new(DecoderFlags::empty());
        let mok2 = first_press(&mut p, b"\x1b[27;2;9~");
        assert_eq!(bare, kitty);
        assert_eq!(bare, mok2);
    }

    #[test]
    fn cross_decoder_enter_matches() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let bare = first_press(&mut p, b"\r");
        let mut p = Decoder::new(DecoderFlags::empty());
        let kitty = first_press(&mut p, b"\x1b[13u");
        let mut p = Decoder::new(DecoderFlags::empty());
        let mok2 = first_press(&mut p, b"\x1b[27;1;13~");
        assert_eq!(bare, kitty);
        assert_eq!(bare, mok2);
    }

    #[test]
    fn cross_decoder_escape_matches() {
        let mut p = Decoder::new(DecoderFlags::empty());
        let kitty = first_press(&mut p, b"\x1b[27u");
        let mut p = Decoder::new(DecoderFlags::empty());
        let mok2 = first_press(&mut p, b"\x1b[27;1;27~");
        assert_eq!(kitty, mok2);
        assert_eq!(kitty.code, KeyCode::Escape);
    }

    /// Decode each encoding through a fresh decoder, return all keys
    /// labelled with their source name, and assert they all compare
    /// equal to the first one.
    fn assert_all_equal(cases: &[(&str, &[u8])]) -> Key {
        assert!(!cases.is_empty());
        let mut decoded: Vec<(&str, Key)> = Vec::with_capacity(cases.len());
        for (name, bytes) in cases {
            let mut p = Decoder::new(DecoderFlags::empty());
            decoded.push((name, first_press(&mut p, bytes)));
        }
        let (first_name, first) = &decoded[0];
        for (name, k) in &decoded[1..] {
            assert_eq!(
                first, k,
                "cross-decoder mismatch: {first_name} ({first:?}) != {name} ({k:?})"
            );
        }
        decoded.into_iter().next().unwrap().1
    }

    fn assert_all_hash_equal(cases: &[(&str, &[u8])]) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hashes: Vec<(&str, u64)> = Vec::new();
        for (name, bytes) in cases {
            let mut p = Decoder::new(DecoderFlags::empty());
            let k = first_press(&mut p, bytes);
            let mut h = DefaultHasher::new();
            k.hash(&mut h);
            hashes.push((name, h.finish()));
        }
        let (first_name, first_hash) = hashes[0];
        for (name, h) in &hashes[1..] {
            assert_eq!(
                first_hash, *h,
                "hash mismatch: {first_name} != {name} ({first_hash:x} vs {h:x})"
            );
        }
    }

    // --- Modifier combinations -----------------------------------------

    #[test]
    fn cross_decoder_ctrl_shift_a_matches() {
        // Bare encoding cannot distinguish Ctrl+Shift+a from Ctrl+a, so
        // it is excluded here intentionally.
        let key = assert_all_equal(&[
            ("kitty all-keys", b"\x1b[97;6u"),
            ("kitty alt-keys", b"\x1b[97:65:97;6u"),
            ("kitty assoc-text", b"\x1b[97;6;65u"),
            ("mok2", b"\x1b[27;6;97~"),
        ]);
        assert_eq!(key.code, KeyCode::Char('a'));
        assert!(key.modifiers.contains(KeyModifiers::CTRL));
        assert!(key.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn cross_decoder_alt_shift_a_matches() {
        let key = assert_all_equal(&[
            ("bare", b"\x1bA"),
            ("kitty all-keys", b"\x1b[97;4u"),
            ("kitty assoc-text", b"\x1b[97;4;65u"),
            ("mok2", b"\x1b[27;4;97~"),
        ]);
        assert_eq!(key.code, KeyCode::Char('a'));
        assert!(key.modifiers.contains(KeyModifiers::ALT));
        assert!(key.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn cross_decoder_ctrl_alt_a_matches() {
        let key = assert_all_equal(&[
            ("bare", b"\x1b\x01"),
            ("kitty", b"\x1b[97;7u"),
            ("mok2", b"\x1b[27;7;97~"),
        ]);
        assert_eq!(key.code, KeyCode::Char('a'));
        assert!(key.modifiers.contains(KeyModifiers::CTRL));
        assert!(key.modifiers.contains(KeyModifiers::ALT));
    }

    #[test]
    fn cross_decoder_ctrl_alt_shift_a_matches() {
        // No bare encoding can express this combo unambiguously.
        let key = assert_all_equal(&[("kitty", b"\x1b[97;8u"), ("mok2", b"\x1b[27;8;97~")]);
        assert!(
            key.modifiers
                .contains(KeyModifiers::CTRL | KeyModifiers::ALT | KeyModifiers::SHIFT)
        );
    }

    #[test]
    fn cross_decoder_super_a_matches() {
        // Super = bit 4 → kitty mod value 9 (1 + 8).
        let key = assert_all_equal(&[
            ("kitty all-keys", b"\x1b[97;9u"),
            ("kitty alt-keys", b"\x1b[97:97:97;9u"),
        ]);
        assert!(key.modifiers.contains(KeyModifiers::SUPER));
    }

    // --- Kitty enhancement-flag variants for the same press -----------

    #[test]
    fn cross_decoder_kitty_shift_a_variants_match() {
        // Same press; every kitty enhancement-flag combo must produce
        // the same binding identity.
        let key = assert_all_equal(&[
            ("plain (mods only)", b"\x1b[97;2u"),
            ("with event-type press", b"\x1b[97;2:1u"),
            ("with alt-keys", b"\x1b[97:65:97;2u"),
            ("with assoc-text", b"\x1b[97;2;65u"),
            ("with alt-keys + assoc-text", b"\x1b[97:65:97;2;65u"),
            ("with alt-keys + event-type press", b"\x1b[97:65:97;2:1u"),
        ]);
        assert_eq!(key.code, KeyCode::Char('a'));
        assert!(key.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn cross_decoder_kitty_repeat_and_release_share_identity() {
        // Event type changes Event variant (Press/Repeat/Release) but
        // the underlying Key identity must stay stable.
        let mut p = Decoder::new(DecoderFlags::empty());
        let press = first_press(&mut p, b"\x1b[97;1:1u");
        let mut p = Decoder::new(DecoderFlags::empty());
        let repeat = match p.parse(b"\x1b[97;1:2u").into_iter().next().unwrap() {
            Event::KeyRepeat(k) => k,
            other => panic!("expected KeyRepeat, got {other:?}"),
        };
        let mut p = Decoder::new(DecoderFlags::empty());
        let release = match p.parse(b"\x1b[97;1:3u").into_iter().next().unwrap() {
            Event::KeyRelease(k) => k,
            other => panic!("expected KeyRelease, got {other:?}"),
        };
        assert_eq!(press, repeat);
        assert_eq!(press, release);
    }

    // --- Scripts beyond Latin/Cyrillic --------------------------------

    #[test]
    fn cross_decoder_greek_alpha_shift_matches() {
        // Α = U+0391, α = U+03B1.
        let key = assert_all_equal(&[
            ("utf8 uppercase", "Α".as_bytes()),
            ("kitty all-keys", b"\x1b[945;2u"),
            ("kitty alt-keys", b"\x1b[945:913;2u"),
        ]);
        assert_eq!(key.code, KeyCode::Char('α'));
        assert!(key.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn cross_decoder_german_eszett_matches() {
        // ß = U+00DF. No simple uppercase mapping that fits a single
        // codepoint, so normalize leaves the code alone.
        let key = assert_all_equal(&[("utf8", "ß".as_bytes()), ("kitty", b"\x1b[223u")]);
        assert_eq!(key.code, KeyCode::Char('ß'));
        assert!(key.modifiers.is_empty());
    }

    #[test]
    fn cross_decoder_hebrew_alef_matches() {
        // א = U+05D0. No case variant.
        let key = assert_all_equal(&[("utf8", "א".as_bytes()), ("kitty", b"\x1b[1488u")]);
        assert_eq!(key.code, KeyCode::Char('א'));
    }

    #[test]
    fn cross_decoder_arabic_alef_matches() {
        // ا = U+0627. No case variant.
        let key = assert_all_equal(&[("utf8", "ا".as_bytes()), ("kitty", b"\x1b[1575u")]);
        assert_eq!(key.code, KeyCode::Char('ا'));
    }

    #[test]
    fn cross_decoder_turkish_dotted_i_matches() {
        // İ = U+0130. Multi-codepoint lowercase ("i\u{307}"), so
        // normalize leaves the code as-is.
        let key = assert_all_equal(&[("utf8", "İ".as_bytes()), ("kitty", b"\x1b[304u")]);
        assert_eq!(key.code, KeyCode::Char('İ'));
    }

    // --- Special / named keys -----------------------------------------

    #[test]
    fn cross_decoder_up_arrow_matches() {
        let key = assert_all_equal(&[
            ("bare CSI A", b"\x1b[A"),
            ("bare CSI 1;1A", b"\x1b[1;1A"),
            ("kitty functional", b"\x1b[57352u"),
        ]);
        assert_eq!(key.code, KeyCode::Up);
        assert!(key.modifiers.is_empty());
    }

    #[test]
    fn cross_decoder_ctrl_up_arrow_matches() {
        let key = assert_all_equal(&[
            ("bare CSI 1;5A", b"\x1b[1;5A"),
            ("kitty functional", b"\x1b[57352;5u"),
        ]);
        assert_eq!(key.code, KeyCode::Up);
        assert!(key.modifiers.contains(KeyModifiers::CTRL));
    }

    #[test]
    fn cross_decoder_page_up_matches() {
        let key = assert_all_equal(&[
            ("bare CSI 5~", b"\x1b[5~"),
            ("kitty functional", b"\x1b[57354u"),
        ]);
        assert_eq!(key.code, KeyCode::PageUp);
    }

    #[test]
    fn cross_decoder_f1_matches() {
        let key = assert_all_equal(&[("ss3", b"\x1bOP"), ("kitty functional", b"\x1b[57364u")]);
        assert_eq!(key.code, KeyCode::F(1));
    }

    #[test]
    fn cross_decoder_f5_matches() {
        let key = assert_all_equal(&[
            ("bare CSI 15~", b"\x1b[15~"),
            ("kitty functional", b"\x1b[57368u"),
        ]);
        assert_eq!(key.code, KeyCode::F(5));
    }

    #[test]
    fn cross_decoder_backspace_matches() {
        let key = assert_all_equal(&[
            ("bare 0x7f", b"\x7f"),
            ("kitty 127", b"\x1b[127u"),
            ("kitty functional", b"\x1b[57347u"),
            ("mok2", b"\x1b[27;1;127~"),
        ]);
        assert_eq!(key.code, KeyCode::Backspace);
    }

    #[test]
    fn cross_decoder_space_matches() {
        let key = assert_all_equal(&[
            ("bare", b" "),
            ("kitty", b"\x1b[32u"),
            ("mok2", b"\x1b[27;1;32~"),
        ]);
        assert_eq!(key.code, KeyCode::Space);
        assert!(key.modifiers.is_empty());
    }

    #[test]
    fn cross_decoder_ctrl_space_matches() {
        let key = assert_all_equal(&[
            ("bare NUL", b"\x00"),
            ("kitty", b"\x1b[32;5u"),
            ("mok2", b"\x1b[27;5;32~"),
        ]);
        assert_eq!(key.code, KeyCode::Space);
        assert!(key.modifiers.contains(KeyModifiers::CTRL));
    }

    #[test]
    fn cross_decoder_delete_matches() {
        let key = assert_all_equal(&[
            ("bare CSI 3~", b"\x1b[3~"),
            ("kitty functional", b"\x1b[57349u"),
        ]);
        assert_eq!(key.code, KeyCode::Delete);
    }

    #[test]
    fn cross_decoder_insert_matches() {
        let key = assert_all_equal(&[
            ("bare CSI 2~", b"\x1b[2~"),
            ("kitty functional", b"\x1b[57348u"),
        ]);
        assert_eq!(key.code, KeyCode::Insert);
    }

    // --- Hash stability across decoders -------------------------------

    #[test]
    fn cross_decoder_hash_stability() {
        // Same press, multiple encodings — must hash to the same value
        // so HashMap<Key, _> lookups never miss.
        assert_all_hash_equal(&[
            ("bare", b"A"),
            ("kitty all-keys", b"\x1b[97;2u"),
            ("kitty alt-keys", b"\x1b[97:65:97;2u"),
            ("kitty assoc-text", b"\x1b[97;2;65u"),
            ("mok2", b"\x1b[27;2;97~"),
        ]);
    }

    #[test]
    fn cross_decoder_alt_shift_tab_is_alt_shift_tab() {
        // `\x1b\x1b[Z` = ESC + (CSI Z). The CSI decoder yields
        // Shift+Tab; the outer ESC promotes it to Alt+Shift+Tab.
        // Kitty/MOK2 produce the same identity.
        let key = assert_all_equal(&[
            ("bare ESC + CSI Z", b"\x1b\x1b[Z"),
            ("kitty Tab+Alt+Shift", b"\x1b[9;4u"),
            ("mok2 Tab+Alt+Shift", b"\x1b[27;4;9~"),
        ]);
        assert_eq!(key.code, KeyCode::Tab);
        assert_eq!(key.modifiers, KeyModifiers::ALT | KeyModifiers::SHIFT);
    }

    // --- Documented pre-existing divergences --------------------------
    //
    // These cases are NOT cross-decoder equal. They reflect limitations
    // of the underlying terminal encodings — the legacy path loses
    // information that the richer kitty/MOK2 paths preserve. We pin
    // them with tests so the behavior is intentional and any future
    // change is deliberate.

    #[test]
    fn divergence_bare_ctrl_shift_letter_collapses_to_ctrl() {
        // Terminals send the same byte for Ctrl+a and Ctrl+Shift+a, so
        // the bare path cannot recover Shift. Richer encodings can.
        let mut p = Decoder::new(DecoderFlags::empty());
        let bare = first_press(&mut p, b"\x01");
        let mut p = Decoder::new(DecoderFlags::empty());
        let kitty = first_press(&mut p, b"\x1b[97;6u");
        assert_eq!(bare.code, KeyCode::Char('a'));
        assert!(bare.modifiers.contains(KeyModifiers::CTRL));
        assert!(!bare.modifiers.contains(KeyModifiers::SHIFT));
        assert!(
            kitty
                .modifiers
                .contains(KeyModifiers::CTRL | KeyModifiers::SHIFT)
        );
        assert_ne!(bare, kitty);
    }

    #[test]
    fn divergence_bare_shifted_symbol_uses_glyph_not_base() {
        // Bare 'Shift+2' is just '@' from the terminal — no way to
        // recover the underlying '2'. Kitty alt-keys preserves both.
        let mut p = Decoder::new(DecoderFlags::empty());
        let bare = first_press(&mut p, b"@");
        let mut p = Decoder::new(DecoderFlags::empty());
        let kitty_alt = first_press(&mut p, b"\x1b[50:64;2u");
        assert_eq!(bare.code, KeyCode::Char('@'));
        assert!(bare.modifiers.is_empty());
        assert_eq!(kitty_alt.code, KeyCode::Char('2'));
        assert!(kitty_alt.modifiers.contains(KeyModifiers::SHIFT));
        assert_ne!(bare, kitty_alt);
    }
}

//! ESC dispatch and Alt-key disambiguation.
//!
//! ## Purpose
//!
//! `ESC` can be a standalone Escape key, an Alt-prefix for a key, or the 7-bit
//! introduction to CSI, SS3, OSC, DCS, APC, SOS, or PM. This module routes the
//! second case byte to the right parser and implements the local Alt-key
//! fallback for printable and control bytes.
//!
//! ```text
//! ESC ─┬─ [ / O / ] / P / _ / X / ^ ─▶ sequence decoder
//!      ├─ printable byte ─────────────▶ Alt+key
//!      ├─ C0 byte or DEL ─────────────▶ Alt+<the key that byte is>
//!      └─ no byte yet ────────────────▶ Incomplete until timeout
//! ```
//!
//! The C0 leg asks the bare mapping what the byte is and adds Alt, rather
//! than naming a key again. One mapping means `ESC CR` and a bare `CR`
//! cannot disagree about which key they are, and that every byte the bare
//! mapping names has an Alt spelling for free. `ESC` is itself a C0 byte, so
//! a run of them takes the same leg. `ESC CR` is the legacy spelling of
//! `alt+enter`.
//!
//! ## Gotchas
//!
//! Runs of multiple `ESC` bytes recurse through the C0 leg. If the inner
//! sequence resolves to a
//! non-Alt key, the outer `ESC` promotes it to Alt; otherwise the outer `ESC` is
//! emitted as a standalone Escape key and the rest of the buffer is retried.
use super::Decoder;
use super::result::ParseResult;
use crate::event::{Event, Key, KeyCode, KeyModifiers};

impl Decoder {
    pub(super) fn parse_escape(&self, buf: &[u8]) -> ParseResult {
        // Base case: a lone `ESC` at the end of the buffer. Wait for a
        // continuation byte; resolve to a standalone `Esc` keypress only
        // once the caller signals the escape timeout has elapsed.
        if buf.len() < 2 {
            return if self.expired {
                ParseResult::Event(Event::KeyPress(self.lone_escape()), 1)
            } else {
                ParseResult::Incomplete
            };
        }

        // `ESC` followed by a non-`ESC` byte: route to the appropriate
        // per-class decoder, fall back to `Alt+<printable>` for plain
        // ASCII, or treat the `ESC` as a standalone keypress.
        match buf[1] {
            b'[' => self.parse_csi(buf),
            b'O' => self.parse_ss3(buf),
            b']' => self.parse_osc(buf),
            b'P' => self.parse_dcs(buf),
            b'_' => self.parse_apc(buf),
            b'X' => self.parse_sos_pm_apc(buf, b'X'),
            b'^' => self.parse_sos_pm_apc(buf, b'^'),
            b if (0x20..0x7f).contains(&b) => {
                let code = if b == b' ' {
                    KeyCode::Space
                } else {
                    KeyCode::Char(b as char)
                };
                ParseResult::Event(
                    Event::KeyPress(Key::new(code, KeyModifiers::ALT).normalized()),
                    2,
                )
            }
            // A C0 byte or DEL: the key that byte is, with Alt.
            //
            // The bare mapping is asked rather than a key named again here.
            // Naming them twice is how `ESC CR` came to disagree with a bare
            // `CR` about which key it was, and asking covers every byte that
            // mapping names rather than the few anyone thought to list.
            // Whatever renames one of them renames both spellings with it:
            // [`DecoderFlags`](crate::event::DecoderFlags) can make `0x09`
            // and `0x0d` into `ctrl+i` and `ctrl+m`, and `0x7f` into
            // `delete`.
            //
            // `ESC` is a C0 byte itself, so a run of them takes this leg as
            // well: the tail parses as its own sequence and comes back to be
            // promoted. An inner key already carrying Alt has nothing to
            // gain from the outer `ESC`, so that one stands alone instead,
            // which is what makes `ESC ESC a` an Escape and then `alt+a`.
            //
            // Among these is `ESC CR`, the legacy spelling of `alt+enter`,
            // which is what a terminal with no Kitty keyboard support sends
            // for it and what an editor binds `shift+enter` to when it has
            // nothing better. A reader pressing Escape and then Enter is
            // separated from it by the escape deadline rather than by a read
            // boundary: a lone `ESC` resolves once the deadline passes, and
            // the source emits it before reading again, so the two meet here
            // only while the deadline is still running. That is the trade
            // `ESC` followed by any printable byte already makes.
            b if b < 0x20 || b == 0x7f => match self.try_parse(&buf[1..]) {
                ParseResult::Event(Event::KeyPress(k), n)
                    if !k.modifiers.contains(KeyModifiers::ALT) =>
                {
                    let key = Key {
                        code: k.code,
                        modifiers: k.modifiers | KeyModifiers::ALT,
                        text: None,
                        shifted_key: k.shifted_key,
                        base_key: k.base_key,
                    };
                    ParseResult::Event(Event::KeyPress(key.normalized()), n + 1)
                }
                ParseResult::Event(_, _) | ParseResult::None(_) => {
                    ParseResult::Event(Event::KeyPress(self.lone_escape()), 1)
                }
                ParseResult::Incomplete => ParseResult::Incomplete,
            },
            _ => ParseResult::Event(Event::KeyPress(self.lone_escape()), 1),
        }
    }
}

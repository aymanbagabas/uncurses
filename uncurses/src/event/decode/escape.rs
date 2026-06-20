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
//!      ├─ control byte ───────────────▶ Alt+Ctrl+key
//!      └─ no byte yet ────────────────▶ Incomplete until timeout
//! ```
//!
//! ## Gotchas
//!
//! Runs of multiple `ESC` bytes recurse. If the inner sequence resolves to a
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
                ParseResult::Event(
                    Event::KeyPress(Key::new(KeyCode::Escape, KeyModifiers::empty()).normalized()),
                    1,
                )
            } else {
                ParseResult::Incomplete
            };
        }

        // `ESC` followed by another `ESC`: decode the inner sequence and
        // try to promote it to `Alt+<key>`. Promotion only applies when
        // the inner result is a key event without `Alt` already set —
        // anything else means the outer `ESC` stands alone.
        if buf[1] == 0x1b {
            return match self.parse_escape(&buf[1..]) {
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
                    ParseResult::Event(Event::KeyPress(key), n + 1)
                }
                ParseResult::Event(_, _) | ParseResult::None(_) => ParseResult::Event(
                    Event::KeyPress(Key::new(KeyCode::Escape, KeyModifiers::empty()).normalized()),
                    1,
                ),
                ParseResult::Incomplete => ParseResult::Incomplete,
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
            // `ESC` followed by a Ctrl-letter byte: Alt+Ctrl+<letter>.
            // Mirrors the bare-byte mapping in `mod.rs::parse_byte` so
            // every decoder path produces the same identity for this
            // combo.
            b @ (0x01..=0x08 | 0x0b..=0x0c | 0x0e..=0x1a) => {
                let c = (b - 1 + b'a') as char;
                ParseResult::Event(
                    Event::KeyPress(
                        Key::new(KeyCode::Char(c), KeyModifiers::ALT | KeyModifiers::CTRL)
                            .normalized(),
                    ),
                    2,
                )
            }
            // `ESC` followed by 0x7f: Alt+Backspace.
            0x7f => ParseResult::Event(
                Event::KeyPress(Key::new(KeyCode::Backspace, KeyModifiers::ALT).normalized()),
                2,
            ),
            _ => ParseResult::Event(
                Event::KeyPress(Key::new(KeyCode::Escape, KeyModifiers::empty()).normalized()),
                1,
            ),
        }
    }
}

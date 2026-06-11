//! SS3 (Single Shift 3) decoder.
//!
//! Format: `ESC O` (or 8-bit `0x8F`) followed by a single final byte.
//! Used by terminals in cursor-key application mode and for keypad
//! keys. Unrecognised finals fall through to legacy-key lookup, then
//! [`Event::UnknownSs3`].

use super::Decoder;
use super::DecoderFlags;
use super::handlers::Ss3;
use super::result::ParseResult;
use super::util::{intro_prefix_len, lookup_legacy_key};
use crate::event::{Event, Key, KeyCode, KeyModifiers};

impl Decoder {
    pub(super) fn parse_ss3(&self, buf: &[u8]) -> ParseResult {
        let prefix_len = intro_prefix_len(buf[0]);
        if buf.len() < prefix_len + 1 {
            return ParseResult::Incomplete;
        }

        let consumed = prefix_len + 1;
        let view = Ss3 {
            final_byte: buf[prefix_len],
        };
        let raw = &buf[..consumed];
        let evt = self
            .handlers
            .dispatch_ss3(view)
            .or_else(|| recognize(view, raw, self.flags))
            .unwrap_or_else(|| Event::UnknownSs3(raw.to_vec()));
        ParseResult::Event(evt, consumed)
    }
}

/// Builtin SS3 recogniser: the keypad / cursor-key final-byte table and
/// the URxvt legacy-key fallback.
fn recognize(view: Ss3, raw_with_intro: &[u8], flags: DecoderFlags) -> Option<Event> {
    let key = match view.final_byte {
        b'A' => Some(KeyCode::Up),
        b'B' => Some(KeyCode::Down),
        b'C' => Some(KeyCode::Right),
        b'D' => Some(KeyCode::Left),
        b'H' => Some(KeyCode::Home),
        b'F' => Some(KeyCode::End),
        b'P' => Some(KeyCode::F(1)),
        b'Q' => Some(KeyCode::F(2)),
        b'R' => Some(KeyCode::F(3)),
        b'S' => Some(KeyCode::F(4)),
        b'M' => Some(KeyCode::KpEnter),
        b'j' => Some(KeyCode::KpMultiply),
        b'k' => Some(KeyCode::KpAdd),
        b'm' => Some(KeyCode::KpSubtract),
        b'n' => Some(KeyCode::KpDecimal),
        b'o' => Some(KeyCode::KpDivide),
        b'p' => Some(KeyCode::Kp0),
        b'q' => Some(KeyCode::Kp1),
        b'r' => Some(KeyCode::Kp2),
        b's' => Some(KeyCode::Kp3),
        b't' => Some(KeyCode::Kp4),
        b'u' => Some(KeyCode::Kp5),
        b'v' => Some(KeyCode::Kp6),
        b'w' => Some(KeyCode::Kp7),
        b'x' => Some(KeyCode::Kp8),
        b'y' => Some(KeyCode::Kp9),
        _ => None,
    };
    if let Some(code) = key {
        return Some(Event::KeyPress(
            Key::new(code, KeyModifiers::empty()).normalized(),
        ));
    }
    lookup_legacy_key(raw_with_intro, flags).map(Event::KeyPress)
}

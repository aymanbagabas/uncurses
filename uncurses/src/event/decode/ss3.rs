//! SS3 (Single Shift 3) decoder.
//!
//! ## Purpose
//!
//! SS3 decodes compact keypad, application-cursor, and function-key sequences
//! into [`Key`](crate::event::Key) events. It also delegates to the legacy-key
//! lookup table for older modifier encodings.
//!
//! ## Wire format
//!
//! SS3 starts with `ESC O` or the 8-bit C1 byte `0x8F`, followed by one final
//! byte. Unknown finals are surfaced as [`Event::UnknownSs3`] with the original
//! framed bytes.
//!
//! ## Gotchas
//!
//! SS3 itself does not carry xterm-style modifier parameters. Modified keys
//! generally arrive through CSI forms unless a legacy table entry matches.
use super::Decoder;
use super::DecoderFlags;
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
        let final_byte = buf[prefix_len];
        let raw = &buf[..consumed];
        let evt = recognize(final_byte, raw, self.flags)
            .unwrap_or_else(|| Event::UnknownSs3(raw.to_vec()));
        ParseResult::Event(evt, consumed)
    }
}

/// Builtin SS3 recogniser: the keypad / cursor-key final-byte table and
/// the URxvt legacy-key fallback.
fn recognize(final_byte: u8, raw_with_intro: &[u8], flags: DecoderFlags) -> Option<Event> {
    let key = match final_byte {
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

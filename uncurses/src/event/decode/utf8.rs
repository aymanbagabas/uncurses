//! UTF-8 multi-byte character decoder for the ground state.
//!
//! Consumes a single 2-4 byte UTF-8 sequence and emits a key press for
//! the resulting character. Invalid leading bytes or malformed
//! sequences yield [`ParseResult::None`] so the caller can advance
//! past a single byte.

use super::Decoder;
use super::result::ParseResult;
use crate::event::{Event, Key, KeyCode};

impl Decoder {
    pub(super) fn parse_utf8(&self, buf: &[u8]) -> ParseResult {
        let first = buf[0];

        // 0x80-0xBF are continuation bytes — invalid as start byte
        if first < 0xC0 {
            return ParseResult::None(1);
        }

        let expected_len = if first < 0xE0 {
            2
        } else if first < 0xF0 {
            3
        } else if first < 0xF8 {
            4
        } else {
            // Invalid UTF-8 start byte (0xF8+)
            return ParseResult::None(1);
        };

        if buf.len() < expected_len {
            return ParseResult::Incomplete;
        }

        match std::str::from_utf8(&buf[..expected_len]) {
            Ok(s) => {
                if let Some(c) = s.chars().next() {
                    let mut key = Key::new(KeyCode::Char(c));
                    crate::event::key::normalize_shift_case(&mut key);
                    ParseResult::Event(Event::KeyPress(key), expected_len)
                } else {
                    ParseResult::None(expected_len)
                }
            }
            Err(_) => ParseResult::None(1),
        }
    }
}

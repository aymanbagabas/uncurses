//! UTF-8 character decoder for the ground state.
//!
//! ## Purpose
//!
//! When the input stream is not inside an escape sequence, bytes at or above
//! `0x80` are parsed here as one UTF-8 scalar value and emitted as a character
//! [`KeyPress`](Event::KeyPress).
//!
//! ## Gotchas
//!
//! Incomplete multi-byte sequences return [`ParseResult::Incomplete`] so the
//! caller can wait for more bytes. Malformed starts or invalid complete
//! sequences return [`ParseResult::None`] and consume a single byte, allowing the
//! outer decoder to recover. Paste bytes do not pass through this decoder.
use super::Decoder;
use super::result::ParseResult;
use crate::event::{Event, Key, KeyCode, KeyModifiers};

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
                    let key = Key::new(KeyCode::Char(c), KeyModifiers::empty()).normalized();
                    ParseResult::Event(Event::KeyPress(key), expected_len)
                } else {
                    ParseResult::None(expected_len)
                }
            }
            Err(_) => ParseResult::None(1),
        }
    }
}

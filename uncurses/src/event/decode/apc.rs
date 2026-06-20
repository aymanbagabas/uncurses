//! APC (Application Program Command) decoder.
//!
//! ## Purpose
//!
//! APC sequences carry application-defined string payloads. The decoder parses
//! the framing, recognizes Kitty graphics replies, and preserves unrecognized
//! payloads as [`Event::UnknownApc`].
//!
//! ## Wire format
//!
//! APC starts with `ESC _` or the 8-bit C1 byte `0x9F`, then a payload, then ST
//! (`ESC \` or `0x9C`). BEL is not a terminator for APC.
//!
//! ## Gotchas
//!
//! Recognized graphics replies are not base64-decoded here; the options are
//! split into key/value strings and the payload bytes are delivered unchanged.
use super::Decoder;
use super::result::ParseResult;
use super::util::{find_string_terminator, intro_prefix_len, parse_kitty_options};
use crate::event::Event;

impl Decoder {
    pub(super) fn parse_apc(&self, buf: &[u8]) -> ParseResult {
        let prefix_len = intro_prefix_len(buf[0]);
        if buf.len() < prefix_len + 1 {
            return ParseResult::Incomplete;
        }
        let (payload_end, st_len) = match find_string_terminator(&buf[prefix_len..]) {
            Some(p) => p,
            None => return ParseResult::Incomplete,
        };
        let payload = &buf[prefix_len..prefix_len + payload_end];
        let consumed = prefix_len + payload_end + st_len;
        let evt = recognize(payload).unwrap_or_else(|| Event::UnknownApc(payload.to_vec()));
        ParseResult::Event(evt, consumed)
    }
}

/// Builtin APC recogniser: currently just the Kitty graphics protocol.
fn recognize(payload: &[u8]) -> Option<Event> {
    // Kitty graphics: APC G <options>;<payload> ST
    let rest = payload.strip_prefix(b"G")?;
    let (opts_bytes, gpayload) = match rest.iter().position(|&b| b == b';') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, &[][..]),
    };
    Some(Event::KittyGraphics {
        options: parse_kitty_options(opts_bytes),
        payload: gpayload.to_vec(),
    })
}

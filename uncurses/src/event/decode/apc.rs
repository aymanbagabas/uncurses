//! APC (Application Program Command) decoder.
//!
//! Format: `ESC _` (or 8-bit `0x9F`) followed by a payload terminated by
//! ST. Used by Kitty graphics protocol; unrecognized payloads surface
//! as [`Event::UnknownApc`].

use super::Decoder;
use super::handlers::Apc;
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
        let view = Apc { payload };
        let evt = self
            .handlers
            .dispatch_apc(view)
            .or_else(|| recognize(view))
            .unwrap_or_else(|| Event::UnknownApc(payload.to_vec()));
        ParseResult::Event(evt, consumed)
    }
}

/// Builtin APC recogniser: currently just the Kitty graphics protocol.
fn recognize(view: Apc<'_>) -> Option<Event> {
    // Kitty graphics: APC G <options>;<payload> ST
    let rest = view.payload.strip_prefix(b"G")?;
    let (opts_bytes, gpayload) = match rest.iter().position(|&b| b == b';') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, &[][..]),
    };
    Some(Event::KittyGraphics {
        options: parse_kitty_options(opts_bytes),
        payload: gpayload.to_vec(),
    })
}

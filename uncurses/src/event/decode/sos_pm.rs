//! SOS (Start of String) and PM (Privacy Message) decoders.
//!
//! ## Purpose
//!
//! SOS and PM use the same string framing as other C1 string controls but have
//! no recognized public payload shapes in this library. The decoder therefore
//! preserves their payload bytes as [`Event::UnknownSos`] or [`Event::UnknownPm`].
//!
//! ## Wire format
//!
//! SOS starts with `ESC X` or `0x98`; PM starts with `ESC ^` or `0x9E`. Both
//! terminate with ST (`ESC \` or `0x9C`). BEL is not a terminator here.
//!
//! ## Gotchas
//!
//! The shared parser entry point receives the 7-bit kind byte (`b'X'` or
//! `b'^'`) even for 8-bit C1 forms so the emitted unknown variant is stable.
use super::Decoder;
use super::result::ParseResult;
use super::util::{find_string_terminator, intro_prefix_len};
use crate::event::Event;

impl Decoder {
    pub(super) fn parse_sos_pm_apc(&self, buf: &[u8], kind: u8) -> ParseResult {
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
        let evt = match kind {
            b'X' => Event::UnknownSos(payload.to_vec()),
            _ => Event::UnknownPm(payload.to_vec()),
        };
        ParseResult::Event(evt, consumed)
    }
}

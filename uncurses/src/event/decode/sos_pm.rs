//! SOS (Start of String) and PM (Privacy Message) decoders.
//!
//! Both have identical wire formats — payload terminated by ST — and
//! no recognized payload shapes, so they surface as
//! [`Event::UnknownSos`] / [`Event::UnknownPm`]. SOS uses `ESC X` or
//! the 8-bit C1 byte `0x98`; PM uses `ESC ^` or `0x9E`.

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
            b'X' => self
                .handlers
                .dispatch_sos(super::handlers::Sos { payload })
                .unwrap_or_else(|| Event::UnknownSos(payload.to_vec())),
            _ => self
                .handlers
                .dispatch_pm(super::handlers::Pm { payload })
                .unwrap_or_else(|| Event::UnknownPm(payload.to_vec())),
        };
        ParseResult::Event(evt, consumed)
    }
}

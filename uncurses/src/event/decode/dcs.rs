//! DCS (Device Control String) decoder.
//!
//! Format: `ESC P` (or 8-bit `0x90`) followed by a payload terminated by
//! ST (`ESC \`) or the 8-bit `0x9C`. Used for terminal capability
//! responses (XTGETTCAP), DECRQSS replies, XTVersion, and tertiary
//! device attributes.

use super::Decoder;
use super::handlers::{self, Dcs};
use super::result::ParseResult;
use super::util::{decode_termcap_payload, find_string_terminator, hex_decode, intro_prefix_len};
use crate::event::Event;

impl Decoder {
    pub(super) fn parse_dcs(&self, buf: &[u8]) -> ParseResult {
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

        let evt = match handlers::split_dcs(payload) {
            Some(view) => self
                .handlers
                .dispatch_dcs(&view)
                .or_else(|| recognize(&view))
                .unwrap_or_else(|| Event::UnknownDcs(payload.to_vec())),
            None => Event::UnknownDcs(payload.to_vec()),
        };
        ParseResult::Event(evt, consumed)
    }
}

/// Builtin DCS recogniser: XTGETTCAP, DECRQSS, XTVersion, tertiary DA.
fn recognize(view: &Dcs<'_>) -> Option<Event> {
    let payload = view.raw;

    // XTGETTCAP response: DCS 1 + r Pt ST  /  DCS 0 + r ST (failure).
    // Pt is `cap_hex=value_hex` pairs separated by `;`. Decode each
    // hex-encoded `cap=value` pair and rebuild a `;`-joined string of
    // the decoded form so consumers see human-readable capabilities.
    if view.params_raw == b"1" && view.intermediates == b"+" && view.final_byte == b'r' {
        return Some(Event::Termcap(decode_termcap_payload(view.data)));
    }

    // DECRQSS response: DCS 1$r ... ST (valid) or DCS 0$r ... ST (invalid).
    // We expose these as Capability as well.
    if (view.params_raw == b"1" || view.params_raw == b"0")
        && view.intermediates == b"$"
        && view.final_byte == b'r'
        && let Ok(s) = std::str::from_utf8(payload)
    {
        return Some(Event::Termcap(s.to_string()));
    }

    // XTVersion reply: DCS > | <name version> ST
    if view.private == Some(b'>') && view.final_byte == b'|' {
        return Some(Event::TerminalVersion(
            String::from_utf8_lossy(view.data).into_owned(),
        ));
    }

    // Tertiary device attributes: DCS ! | <hex-id> ST. The payload is a
    // hex-encoded byte string identifying the terminal; decode it so the
    // event carries the raw identifier bytes (as a UTF-8 string when
    // possible, otherwise the lossy decoding).
    if view.intermediates == b"!" && view.final_byte == b'|' {
        let decoded = match hex_decode(view.data) {
            Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            None => String::from_utf8_lossy(view.data).into_owned(),
        };
        return Some(Event::TertiaryDeviceAttributes(decoded));
    }

    None
}

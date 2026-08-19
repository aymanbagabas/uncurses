//! DCS (Device Control String) decoder.
//!
//! ## Purpose
//!
//! DCS sequences carry terminal-control reply strings. This decoder recognizes
//! capability replies, DECRPSS status replies, terminal-name replies, and
//! tertiary device attributes, while preserving unknown payloads as
//! [`Event::UnknownDcs`].
//!
//! ## Wire format
//!
//! DCS starts with `ESC P` or the 8-bit C1 byte `0x90`, then a payload, then ST
//! (`ESC \` or `0x9C`). BEL is intentionally not accepted as a terminator in
//! DCS payloads.
//!
//! ## Gotchas
//!
//! XTGETTCAP success and failure replies both decode their hex payloads; the
//! [`Event::Termcap`] `recognized` field carries the success bit. Malformed hex entries are skipped rather than
//! making the whole reply fail.
use super::Decoder;
use super::result::ParseResult;
use super::util::{decode_termcap_payload, find_string_terminator, hex_decode, intro_prefix_len};
use crate::event::{Event, SettingReport};

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

        let evt = recognize(payload).unwrap_or_else(|| Event::UnknownDcs(payload.to_vec()));
        ParseResult::Event(evt, consumed)
    }
}

/// Builtin DCS recogniser: XTGETTCAP, DECRPSS, XTVersion, tertiary DA.
///
/// Splits the payload into its private prefix / parameter / intermediate /
/// final-byte / data regions and matches the known reply shapes. Returns
/// `None` for anything unrecognised or malformed so the caller falls back
/// to [`Event::UnknownDcs`].
fn recognize(payload: &[u8]) -> Option<Event> {
    let (private, head_start) = match payload.first() {
        Some(&b) if matches!(b, b'?' | b'<' | b'>' | b'=') => (Some(b), 1),
        _ => (None, 0),
    };
    let mut i = head_start;
    let final_pos = loop {
        let b = *payload.get(i)?;
        if (0x40..=0x7e).contains(&b) {
            break i;
        }
        if !((0x30..=0x3f).contains(&b) || (0x20..=0x2f).contains(&b) || b == b';' || b == b':') {
            return None;
        }
        i += 1;
    };
    let head = &payload[head_start..final_pos];
    let mid = head
        .iter()
        .position(|&x| (0x20..=0x2f).contains(&x))
        .unwrap_or(head.len());
    let (params_raw, intermediates) = head.split_at(mid);
    let final_byte = payload[final_pos];
    let data = &payload[final_pos + 1..];

    // XTGETTCAP response: DCS 1 + r Pt ST (valid) / DCS 0 + r Pt ST
    // (failure). Pt is `cap_hex=value_hex` pairs separated by `;`. The
    // payload is decoded the same way in both cases (a failure echoes the
    // requested, now known-unsupported, cap names); `recognized` carries
    // the 1-vs-0 distinction so a failure is reported rather than dropped.
    if intermediates == b"+" && final_byte == b'r' && (params_raw == b"1" || params_raw == b"0") {
        return Some(Event::Termcap {
            recognized: params_raw == b"1",
            entries: decode_termcap_payload(data),
        });
    }

    // DECRPSS, the reply to DECRQSS: DCS 1$r <D...D> ST (valid) or
    // DCS 0$r ST (invalid). Reported separately from XTGETTCAP: the data is
    // a settings string, not `cap=value` pairs.
    //
    // Only 0 and 1 are ever sent, but do not "correct" which is which. The
    // VT510 manual documents 0 as valid and 1 as invalid, and it is wrong:
    // testing a VT420 in 1996 showed the two reversed, and vttest, DEC STD
    // 070 and xterm all agree that 1 is the valid one.
    if (params_raw == b"1" || params_raw == b"0") && intermediates == b"$" && final_byte == b'r' {
        return Some(Event::SettingReport(if params_raw == b"1" {
            SettingReport::Raw(String::from_utf8_lossy(data).into_owned())
        } else {
            SettingReport::Refused
        }));
    }

    // XTVersion reply: DCS > | <name version> ST
    if private == Some(b'>') && final_byte == b'|' {
        return Some(Event::TerminalName(
            String::from_utf8_lossy(data).into_owned(),
        ));
    }

    // Tertiary device attributes: DCS ! | <hex-id> ST. The payload is a
    // hex-encoded byte string identifying the terminal; decode it so the
    // event carries the raw identifier bytes (as a UTF-8 string when
    // possible, otherwise the lossy decoding).
    if intermediates == b"!" && final_byte == b'|' {
        let decoded = match hex_decode(data) {
            Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            None => String::from_utf8_lossy(data).into_owned(),
        };
        return Some(Event::TertiaryDeviceAttributes(decoded));
    }

    None
}

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
use super::util::{
    ControlSequence, decode_termcap_payload, find_string_terminator, hex_decode, intro_prefix_len,
};
use crate::ansi::cursor::CursorStyle;
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
/// Reads the command section with [`ControlSequence`] and matches the known
/// reply shapes. Returns `None` for anything unrecognised or malformed so
/// the caller falls back to [`Event::UnknownDcs`].
fn recognize(payload: &[u8]) -> Option<Event> {
    // A DCS carries a command section shaped exactly like a CSI body, then
    // its payload, so the same parser reads the header here and the setting
    // a DECRPSS reply spells out below.
    let (seq, data) = ControlSequence::parse(payload)?;
    let private = seq.private();
    let params_raw = seq.params().raw();
    let intermediate = seq.intermediate();
    let final_byte = seq.final_byte();

    // XTGETTCAP response: DCS 1 + r Pt ST (valid) / DCS 0 + r Pt ST
    // (failure). Pt is `cap_hex=value_hex` pairs separated by `;`. The
    // payload is decoded the same way in both cases (a failure echoes the
    // requested, now known-unsupported, cap names); `recognized` carries
    // the 1-vs-0 distinction so a failure is reported rather than dropped.
    if intermediate == Some(b'+')
        && final_byte == b'r'
        && (params_raw == b"1" || params_raw == b"0")
    {
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
    if (params_raw == b"1" || params_raw == b"0")
        && intermediate == Some(b'$')
        && final_byte == b'r'
    {
        return Some(Event::SettingReport(if params_raw == b"1" {
            recognize_setting(data)
        } else {
            SettingReport::Unrecognized
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
    if intermediate == Some(b'!') && final_byte == b'|' {
        let decoded = match hex_decode(data) {
            Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            None => String::from_utf8_lossy(data).into_owned(),
        };
        return Some(Event::TertiaryDeviceAttributes(decoded));
    }

    None
}

/// Decode the setting a valid DECRPSS reply carries.
///
/// The reply spells the control function out the way a CSI would, without
/// the introducer, so `2 q` is the terminal reporting a steady block for
/// DECSCUSR. Anything this does not recognize is handed back verbatim.
fn recognize_setting(data: &[u8]) -> SettingReport {
    let raw = || SettingReport::Raw(String::from_utf8_lossy(data).into_owned());
    let Some((seq, rest)) = ControlSequence::parse(data) else {
        return raw();
    };
    // A setting is the whole reply. Anything left over means this was read
    // as something it is not.
    if !rest.is_empty() {
        return raw();
    }
    match (seq.private(), seq.intermediate(), seq.final_byte()) {
        // DECSCUSR: CSI Ps SP q. A reply states the current setting, so an
        // omitted parameter is not expected here; read as 0, the terminal
        // default. Terminals do not agree on that reading when *setting* a
        // style: kitty and Konsole take a bare `CSI SP q` as a blinking
        // block, while foot, VTE and Windows Terminal take it as 0.
        (None, Some(b' '), b'q') => CursorStyle::from_param(seq.params().get_or(0, 0))
            .map_or_else(raw, SettingReport::CursorStyle),
        _ => raw(),
    }
}

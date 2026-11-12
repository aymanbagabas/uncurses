//! OSC (Operating System Command) decoder.
//!
//! ## Purpose
//!
//! OSC sequences carry host/terminal data such as color replies, palette
//! entries, and clipboard replies. The decoder recognizes the reply forms used
//! by the public event API and preserves other payloads as [`Event::UnknownOsc`].
//!
//! ## Wire format
//!
//! OSC starts with `ESC ]` or the 8-bit C1 byte `0x9D`, then a payload, then
//! BEL (`0x07`), 7-bit ST (`ESC \`), or 8-bit ST (`0x9C`). OSC is the only
//! string class in this decoder that accepts BEL as a terminator.
//!
//! ## Gotchas
//!
//! OSC 52 clipboard content is surfaced as the wire payload string; decoding or
//! interpreting that content is left to callers. Color channels with 1-4 hex
//! digits are scaled down to 8-bit RGB.
use super::Decoder;
use super::result::ParseResult;
use super::util::{find_string_terminator, intro_prefix_len};
use crate::color::Color;
use crate::event::{ClipboardSelection, Event, ProgramStatusCapabilities};

impl Decoder {
    pub(super) fn parse_osc(&self, buf: &[u8]) -> ParseResult {
        let prefix_len = intro_prefix_len(buf[0]);
        if buf.len() == prefix_len {
            return ParseResult::Incomplete;
        }
        let (payload_end, st_len) = match find_string_terminator(&buf[prefix_len..], true) {
            Some(p) => p,
            None => return ParseResult::IncompleteString,
        };
        let payload = &buf[prefix_len..prefix_len + payload_end];
        ParseResult::Event(finalize_osc(payload), prefix_len + payload_end + st_len)
    }
}

/// Resolve an OSC payload: the builtin recogniser, then
/// [`Event::UnknownOsc`] as a last resort.
fn finalize_osc(payload: &[u8]) -> Event {
    recognize(payload).unwrap_or_else(|| Event::UnknownOsc(payload.to_vec()))
}

/// Builtin OSC recogniser. Returns `None` for unrecognised payloads so the
/// caller can decide the fallback.
fn recognize(payload: &[u8]) -> Option<Event> {
    // Split the leading numeric command from the rest.
    let semi = payload.iter().position(|&b| b == b';')?;
    let cmd_bytes = &payload[..semi];
    let rest = &payload[semi + 1..];
    let cmd: u32 = std::str::from_utf8(cmd_bytes).ok()?.parse().ok()?;

    match cmd {
        4 => parse_osc_palette_color(rest),
        10 => parse_osc_color(rest).map(Event::ForegroundColor),
        11 => parse_osc_color(rest).map(Event::BackgroundColor),
        12 => parse_osc_color(rest).map(Event::CursorColor),
        52 => parse_osc_clipboard(rest),
        72 => parse_drag_drop(rest),
        7501 => parse_osc_program_status(rest),
        _ => None,
    }
}

fn parse_drag_drop(rest: &[u8]) -> Option<Event> {
    let (metadata, payload) = match rest.iter().position(|&b| b == b';') {
        Some(index) => (&rest[..index], &rest[index + 1..]),
        None => (rest, &[][..]),
    };
    if payload.len() > 4096 {
        return None;
    }
    let metadata = std::str::from_utf8(metadata).ok()?;
    let options = metadata
        .split(':')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=')?;
            if key.is_empty() || pair.chars().any(char::is_control) {
                return None;
            }
            Some((key.to_owned(), value.to_owned()))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Event::DragDrop {
        options,
        payload: payload.to_vec(),
    })
}

fn parse_osc_program_status(s: &[u8]) -> Option<Event> {
    s.strip_prefix(b"?")?;
    let mut states = ["idle", "working", "done", "blocked", "error"]
        .map(String::from)
        .to_vec();
    let mut kinds = ["permission", "question", "auth"]
        .map(String::from)
        .to_vec();
    for pair in s.split(|&b| b == b':').skip(1) {
        let Ok(pair) = std::str::from_utf8(pair) else {
            continue;
        };
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let list = match key.trim() {
            "states" => &mut states,
            "kinds" => &mut kinds,
            _ => continue,
        };
        let value = value.trim();
        if !crate::ansi::program_status::valid_value(value) {
            continue;
        }
        *list = value
            .split(',')
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
    }
    Some(Event::ProgramStatus(ProgramStatusCapabilities {
        states,
        kinds,
    }))
}

/// Parse an OSC 4 palette reply body: `<index>;<color>` where `color` is
/// an xterm `rgb:` value. Returns `None` if unrecognized.
fn parse_osc_palette_color(s: &[u8]) -> Option<Event> {
    let semi = s.iter().position(|&b| b == b';')?;
    let index: u8 = std::str::from_utf8(&s[..semi]).ok()?.parse().ok()?;
    let color = parse_osc_color(&s[semi + 1..])?;
    Some(Event::PaletteColor { index, color })
}

/// Parse an OSC color value like `rgb:RRRR/GGGG/BBBB` (xterm common form) or
/// `rgb:RR/GG/BB`. Returns `None` if unrecognized.
fn parse_osc_color(s: &[u8]) -> Option<Color> {
    let s = std::str::from_utf8(s).ok()?;
    let s = s.strip_prefix("rgb:")?;
    let mut parts = s.split('/');
    let r = parse_hex_channel(parts.next()?)?;
    let g = parse_hex_channel(parts.next()?)?;
    let b = parse_hex_channel(parts.next()?)?;
    Some(Color::Rgb(r, g, b))
}

/// Parse a 1–4 hex-digit channel value, scaling down to a u8.
fn parse_hex_channel(s: &str) -> Option<u8> {
    if s.is_empty() || s.len() > 4 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    let scaled = match s.len() {
        1 => (v * 0x11) as u8,
        2 => v as u8,
        3 => (v >> 4) as u8,
        4 => (v >> 8) as u8,
        _ => unreachable!(),
    };
    Some(scaled)
}

/// Parse an OSC 52 payload: `<selection>;<base64-content-or-?>`.
fn parse_osc_clipboard(s: &[u8]) -> Option<Event> {
    let semi = s.iter().position(|&b| b == b';')?;
    let sel_bytes = &s[..semi];
    let content = &s[semi + 1..];
    let selection = match sel_bytes.first().copied() {
        Some(b'c') => ClipboardSelection::System,
        Some(b'p') => ClipboardSelection::Primary,
        Some(c) => ClipboardSelection::Other(c as char),
        None => ClipboardSelection::System,
    };
    let content = std::str::from_utf8(content).ok()?.to_string();
    Some(Event::Clipboard { selection, content })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::DecoderFlags;

    #[test]
    fn drag_drop_messages_preserve_metadata_and_wire_payloads() {
        for intro in [b"\x1b]".as_slice(), b"\x9d"] {
            for ending in [b"\x07".as_slice(), b"\x1b\\", b"\x9c"] {
                for (body, options, payload) in [
                    (b"t=q".as_slice(), vec![("t", "q")], b"".as_slice()),
                    (
                        b"t=r:x=1:i=42:m=1:future=value;YW",
                        vec![
                            ("t", "r"),
                            ("x", "1"),
                            ("i", "42"),
                            ("m", "1"),
                            ("future", "value"),
                        ],
                        b"YW",
                    ),
                    (b"m=0;Jj", vec![("m", "0")], b"Jj"),
                    (b"t=r:x=1;", vec![("t", "r"), ("x", "1")], b""),
                    (b"t=R;EIO:unavailable", vec![("t", "R")], b"EIO:unavailable"),
                ] {
                    let wire = [intro, b"72;", body, ending].concat();
                    let expected = Event::DragDrop {
                        options: options
                            .iter()
                            .map(|(k, v)| ((*k).into(), (*v).into()))
                            .collect(),
                        payload: payload.to_vec(),
                    };
                    for split in 0..=wire.len() {
                        let mut decoder = Decoder::default();
                        let mut events = decoder.parse(&wire[..split]);
                        events.extend(decoder.parse(&wire[split..]));
                        assert_eq!(events, std::slice::from_ref(&expected), "split {split}");
                    }
                }
            }
        }
    }

    #[test]
    fn drag_drop_invalid_messages_remain_unknown() {
        for body in [b"72;t".as_slice(), b"72;=r;", b"72;t=\xff;", b"72;t=r\n;"] {
            let mut decoder = Decoder::default();
            assert_eq!(
                decoder.parse(&[b"\x1b]".as_slice(), body, b"\x1b\\"].concat()),
                [Event::UnknownOsc(body.to_vec())]
            );
        }
        let body = [b"72;t=r;".as_slice(), &[b'A'; 4097]].concat();
        assert_eq!(
            Decoder::default().parse(&[b"\x1b]".as_slice(), &body, b"\x1b\\"].concat()),
            [Event::UnknownOsc(body)]
        );
    }

    #[test]
    fn program_status_support_reply_accepts_extensions_and_split_input() {
        for intro in [b"\x1b]".as_slice(), b"\x9d"] {
            for ending in [b"\x07".as_slice(), b"\x1b\\", b"\x9c"] {
                for (body, states, kinds) in [
                    (
                        b"7501;?".as_slice(),
                        ["idle", "working", "done", "blocked", "error"].as_slice(),
                        ["permission", "question", "auth"].as_slice(),
                    ),
                    (
                        b"7501;?:future=value",
                        &["idle", "working", "done", "blocked", "error"],
                        &["permission", "question", "auth"],
                    ),
                    (
                        b"7501;?:states=idle,paused:kinds=question,confirm:future=value",
                        &["idle", "paused"],
                        &["question", "confirm"],
                    ),
                ] {
                    let wire = [intro, body, ending].concat();
                    for split in 0..=wire.len() {
                        let mut decoder = Decoder::new(DecoderFlags::empty());
                        let mut events = decoder.parse(&wire[..split]);
                        events.extend(decoder.parse(&wire[split..]));
                        assert_eq!(
                            events,
                            [Event::ProgramStatus(ProgramStatusCapabilities {
                                states: states.iter().map(|s| (*s).to_owned()).collect(),
                                kinds: kinds.iter().map(|s| (*s).to_owned()).collect(),
                            })]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn program_status_reply_lists_replace_defaults_independently() {
        for (body, states, kinds) in [
            (
                b"?:states=done".as_slice(),
                ["done"].as_slice(),
                ["permission", "question", "auth"].as_slice(),
            ),
            (
                b"?:kinds=auth",
                &["idle", "working", "done", "blocked", "error"],
                &["auth"],
            ),
            (b"?:states=:kinds=", &[], &[]),
            (
                b"? : states = idle,done : kinds = auth : ignored=value",
                &["idle", "done"],
                &["auth"],
            ),
            (
                b"?:states=working:states=done:kinds=auth:kinds=",
                &["done"],
                &[],
            ),
            (
                b"?:states=done:states=not valid:kinds=auth:kinds=\xff:broken",
                &["done"],
                &["auth"],
            ),
            (
                b"?:states=,done,,paused,:kinds=,,",
                &["done", "paused"],
                &[],
            ),
        ] {
            let mut decoder = Decoder::new(DecoderFlags::empty());
            let wire = [b"\x1b]7501;".as_slice(), body, b"\x1b\\"].concat();
            assert_eq!(
                decoder.parse(&wire),
                [Event::ProgramStatus(ProgramStatusCapabilities {
                    states: states.iter().map(|s| (*s).to_owned()).collect(),
                    kinds: kinds.iter().map(|s| (*s).to_owned()).collect(),
                })]
            );
        }
    }

    #[test]
    fn program_status_reports_and_other_oscs_remain_unknown() {
        for body in [
            b"7501;state=done".as_slice(),
            b"7501;",
            b"7501;yes",
            b"7502;?",
        ] {
            let mut decoder = Decoder::new(DecoderFlags::empty());
            let wire = [b"\x1b]".as_slice(), body, b"\x1b\\"].concat();
            assert_eq!(decoder.parse(&wire), [Event::UnknownOsc(body.to_vec())]);
        }
    }
}

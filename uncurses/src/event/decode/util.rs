//! Shared helpers for per-sequence decoders.
//!
//! ## Purpose
//!
//! This module contains stateless parsing utilities used by CSI, DCS, OSC, APC,
//! SS3, and Windows-input decoders: hex decoding, string terminator search,
//! modifier decoding, legacy-key lookup, and small key/event construction
//! helpers.
//!
//! ## Key helpers
//!
//! * [`Params`] is re-exported for lazy CSI-style parameter access.
//! * [`is_c1_introducer`] identifies the 8-bit C1 bytes that begin string or
//!   control sequences.
//! * [`lookup_legacy_key`] covers older terminal key encodings that do not fit
//!   the modern CSI parameter grammar.
//!
//! ## Gotchas
//!
//! These helpers deliberately do not inspect [`Decoder`](super::Decoder) state.
//! Functions that need paste mode, UTF-8 mouse mode, pending event queues, or
//! Windows surrogate state live on the decoder impls instead.
use super::result::ParseResult;
use crate::event::{Event, Key, KeyCode, KeyModifiers, decode::DecoderFlags};

pub(super) use crate::ansi::params::{ControlSequence, Params};

/// Decode an ASCII hex byte string (e.g. `"61"` → `b"a"`). Returns `None` if
/// the input has an odd length or contains a non-hex byte.
pub(super) fn hex_decode(data: &[u8]) -> Option<Vec<u8>> {
    if !data.len().is_multiple_of(2) {
        return None;
    }
    let nib = |b: u8| -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(data.len() / 2);
    for pair in data.as_chunks::<2>().0 {
        out.push((nib(pair[0])? << 4) | nib(pair[1])?);
    }
    Some(out)
}

/// Decode an XTGETTCAP payload: a `;`-separated list of `cap_hex=value_hex`
/// pairs (value may be omitted). Pairs whose name or value fails to decode
/// are skipped. Raw bytes are preserved via lossy UTF-8 conversion.
///
/// The pairs are returned separately rather than rejoined into one string.
/// Only the hex form is delimiter-safe: decoded values routinely contain `;`
/// and `=` themselves (`kf13` is `\E[1;2P`), so rejoining them would be
/// ambiguous and any later split would invent capabilities that were never
/// reported.
pub(super) fn decode_termcap_payload(data: &[u8]) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for entry in data.split(|&b| b == b';') {
        let mut parts = entry.splitn(2, |&b| b == b'=');
        let name_hex = parts.next().unwrap_or(&[]);
        let value_hex = parts.next();
        let name = match hex_decode(name_hex) {
            Some(bytes) if !bytes.is_empty() => String::from_utf8_lossy(&bytes).into_owned(),
            _ => continue,
        };
        let value = match value_hex {
            Some(v) => match hex_decode(v) {
                Some(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
                None => continue,
            },
            None => None,
        };
        out.push((name, value));
    }
    out
}

/// Number of bytes that the introducer at `b0` occupies.
///
/// Returns 2 for the 7-bit `ESC X` form and 1 for the 8-bit C1 single-byte
/// form. Callers pass the first byte of a sequence they have already classified
/// as a 7-bit or 8-bit introducer.
pub(super) fn intro_prefix_len(b0: u8) -> usize {
    if b0 == 0x1b { 2 } else { 1 }
}

/// Returns `true` if `b` is one of the 8-bit C1 control bytes that
/// introduces a string or control sequence (SS3, DCS, SOS, CSI, OSC, PM, APC).
pub(crate) fn is_c1_introducer(b: u8) -> bool {
    matches!(b, 0x8f | 0x90 | 0x98 | 0x9b | 0x9d | 0x9e | 0x9f)
}

/// Frame a control string and pass its complete body to `finalize`.
///
/// Recognizes `ESC \` and standalone `0x9C`. Only OSC enables BEL.
/// Another Escape sequence interrupts the string and stays unconsumed.
/// A trailing Escape waits for the next byte to distinguish these cases.
/// Complete UTF-8 characters stay in the payload; incomplete characters wait
/// for more input. Invalid bytes advance one byte at a time.
pub(super) fn parse_string(
    data: &[u8],
    allow_bel: bool,
    finalize: impl FnOnce(&[u8]) -> Event,
) -> ParseResult {
    let prefix_len = intro_prefix_len(data[0]);
    if data.len() == prefix_len {
        return ParseResult::Incomplete;
    }
    let mut i = prefix_len;
    while i < data.len() {
        let b = data[i];
        if b == 0x9c || (allow_bel && b == 0x07) {
            return ParseResult::Event(finalize(&data[prefix_len..i]), i + 1);
        }
        if b == 0x1b {
            return match data.get(i + 1) {
                Some(b'\\') => ParseResult::Event(finalize(&data[prefix_len..i]), i + 2),
                Some(_) => ParseResult::Event(Event::Unknown(data[..i].to_vec()), i),
                None => ParseResult::IncompleteString,
            };
        }
        let width = match b {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => 1,
        };
        if width > 1 {
            let end = (i + width).min(data.len());
            match std::str::from_utf8(&data[i..end]) {
                Ok(_) => {
                    i = end;
                    continue;
                }
                Err(err) if err.error_len().is_none() => return ParseResult::IncompleteString,
                Err(_) => {}
            }
        }
        i += 1;
    }
    ParseResult::IncompleteString
}

/// Parse a Kitty Graphics options string like `a=T,f=32,s=10,v=5`.
pub(super) fn parse_kitty_options(s: &[u8]) -> Vec<(String, String)> {
    let s = match std::str::from_utf8(s) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    if s.is_empty() {
        return Vec::new();
    }
    s.split(',')
        .filter_map(|kv| {
            let mut it = kv.splitn(2, '=');
            let k = it.next()?.to_string();
            let v = it.next().unwrap_or("").to_string();
            if k.is_empty() { None } else { Some((k, v)) }
        })
        .collect()
}

pub(super) fn csi_modifiers(params: Params<'_>) -> KeyModifiers {
    // Modifier param defaults to 1 (= no modifiers) per the xterm protocol.
    xterm_modifiers(params.get_or(1, 1) as u16)
}

/// Read the Kitty keyboard-protocol event-type sub-parameter from the
/// modifiers group (`params[1]:1`). Defaults to 1 (press) when absent
/// or when the host terminal isn't reporting event types.
///
/// Encoding: `1` = press, `2` = repeat, `3` = release.
pub(super) fn csi_kitty_phase(params: Params<'_>) -> u32 {
    params.group(1).and_then(|g| g.nth(1)).unwrap_or(1)
}

/// Wrap a [`Key`] into [`Event::KeyPress`], [`Event::KeyRepeat`], or
/// [`Event::KeyRelease`] based on a Kitty event-type code (see
/// [`csi_kitty_phase`]). Unknown phases collapse to press.
pub(super) fn key_event_for_phase(key: Key, phase: u32) -> Event {
    match phase {
        2 => Event::KeyRepeat(key),
        3 => Event::KeyRelease(key),
        _ => Event::KeyPress(key),
    }
}

/// Look up a legacy escape sequence (URxvt-style modifier suffixes, lowercase
/// arrows, etc.) and return the corresponding `Key`. Returns `None` if the
/// sequence isn't in the legacy table.
pub(super) fn lookup_legacy_key(seq: &[u8], flags: DecoderFlags) -> Option<Key> {
    use KeyCode::*;
    // URxvt lowercase shift+arrows: ESC [ a/b/c/d
    if seq.len() == 3 && seq[0] == 0x1b && seq[1] == b'[' {
        let code = match seq[2] {
            b'a' => Some(Up),
            b'b' => Some(Down),
            b'c' => Some(Right),
            b'd' => Some(Left),
            _ => None,
        };
        if let Some(c) = code {
            return Some(Key::new(c, KeyModifiers::SHIFT).normalized());
        }
    }
    // URxvt ctrl+arrows: ESC O a/b/c/d
    if seq.len() == 3 && seq[0] == 0x1b && seq[1] == b'O' {
        let code = match seq[2] {
            b'a' => Some(Up),
            b'b' => Some(Down),
            b'c' => Some(Right),
            b'd' => Some(Left),
            _ => None,
        };
        if let Some(c) = code {
            return Some(Key::new(c, KeyModifiers::CTRL).normalized());
        }
    }
    // URxvt modifier-suffix CSI ~ keys: ESC [ N <suffix>
    //   $ → Shift, ^ → Ctrl, @ → Shift+Ctrl
    if seq.len() >= 4 && seq[0] == 0x1b && seq[1] == b'[' {
        let suffix = *seq.last().unwrap();
        let mods = match suffix {
            b'$' => Some(KeyModifiers::SHIFT),
            b'^' => Some(KeyModifiers::CTRL),
            b'@' => Some(KeyModifiers::SHIFT | KeyModifiers::CTRL),
            _ => None,
        };
        if let Some(m) = mods {
            // Numeric portion is between `[` and the suffix.
            let num = std::str::from_utf8(&seq[2..seq.len() - 1]).ok()?;
            let n: u16 = num.parse().ok()?;
            let code = tilde_code_to_keycode(n).map(|kc| remap_tilde(n, kc, flags))?;
            return Some(Key::new(code, m).normalized());
        }
    }
    None
}

pub(super) fn tilde_code_to_keycode(n: u16) -> Option<KeyCode> {
    use KeyCode::*;
    Some(match n {
        1 | 7 => Home,
        2 => Insert,
        3 => Delete,
        4 | 8 => End,
        5 => PageUp,
        6 => PageDown,
        11 => F(1),
        12 => F(2),
        13 => F(3),
        14 => F(4),
        15 => F(5),
        17 => F(6),
        18 => F(7),
        19 => F(8),
        20 => F(9),
        21 => F(10),
        23 => F(11),
        24 => F(12),
        25 => F(13),
        26 => F(14),
        28 => F(15),
        29 => F(16),
        31 => F(17),
        32 => F(18),
        33 => F(19),
        34 => F(20),
        _ => return None,
    })
}

/// Apply [`DecoderFlags::FIND_KEY`] / [`DecoderFlags::SELECT_KEY`] swaps to a
/// keycode resolved from a tilde-numeric code. Codes 1 and 4 only — the
/// aliases 7 and 8 always resolve to plain Home/End.
pub(super) fn remap_tilde(n: u16, code: KeyCode, flags: DecoderFlags) -> KeyCode {
    match (n, code) {
        (1, KeyCode::Home) if flags.contains(DecoderFlags::FIND_KEY) => KeyCode::Find,
        (4, KeyCode::End) if flags.contains(DecoderFlags::SELECT_KEY) => KeyCode::Select,
        _ => code,
    }
}

pub(super) fn xterm_modifiers(n: u16) -> KeyModifiers {
    let n = n.saturating_sub(1);
    let mut mods = KeyModifiers::empty();
    if n & 1 != 0 {
        mods |= KeyModifiers::SHIFT;
    }
    if n & 2 != 0 {
        mods |= KeyModifiers::ALT;
    }
    if n & 4 != 0 {
        mods |= KeyModifiers::CTRL;
    }
    if n & 8 != 0 {
        mods |= KeyModifiers::META;
    }
    mods
}

pub(super) fn key_with_mods(code: KeyCode, mods: KeyModifiers) -> Key {
    Key::new(code, mods).normalized()
}

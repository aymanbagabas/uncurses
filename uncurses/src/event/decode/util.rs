//! Shared parsing helpers used across the per-sequence decoders.
//!
//! Pure functions only — no `Decoder` state. Anything that needs
//! to look at decoder state stays in [`super`].

use crate::event::{Key, KeyCode, KeyModifiers, decode::DecoderFlags};

pub(super) use crate::ansi::params::Params;

/// Decode an ASCII hex byte string (e.g. `"61"` → `b"a"`). Returns `None` if
/// the input has an odd length or contains a non-hex byte.
pub(super) fn hex_decode(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() % 2 != 0 {
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
    for pair in data.chunks_exact(2) {
        out.push((nib(pair[0])? << 4) | nib(pair[1])?);
    }
    Some(out)
}

/// Decode an XTGETTCAP payload: a `;`-separated list of `cap_hex=value_hex`
/// pairs (value may be omitted). Pairs whose name fails to decode are
/// skipped; pairs whose value fails to decode are also skipped. The result
/// is a `;`-joined string of `cap[=value]` entries with raw bytes preserved
/// via lossy UTF-8 conversion.
pub(super) fn decode_termcap_payload(data: &[u8]) -> String {
    let mut out = String::new();
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
        if !out.is_empty() {
            out.push(';');
        }
        out.push_str(&name);
        if let Some(v) = value {
            if !v.is_empty() {
                out.push('=');
                out.push_str(&v);
            }
        }
    }
    out
}

/// Locate a string terminator (BEL or ESC `\`) inside `data`. Returns
/// `(byte_offset_of_terminator, terminator_length)`.
/// Number of bytes that the introducer at `b0` occupies. Returns 2 for the
/// 7-bit `ESC X` form and 1 for the 8-bit C1 single-byte form.
pub(super) fn intro_prefix_len(b0: u8) -> usize {
    if b0 == 0x1b { 2 } else { 1 }
}

/// Returns `true` if `b` is one of the 8-bit C1 control bytes that
/// introduces a string or control sequence (SS3, DCS, SOS, CSI, OSC, PM, APC).
pub(crate) fn is_c1_introducer(b: u8) -> bool {
    matches!(b, 0x8f | 0x90 | 0x98 | 0x9b | 0x9d | 0x9e | 0x9f)
}

/// Search `data` for a string terminator and return `(payload_end, st_len)`.
///
/// Recognises both the 7-bit form `ESC \` (2 bytes) and the 8-bit form
/// `0x9C` (1 byte). BEL is intentionally NOT accepted here — it is a
/// terminator for OSC sequences only and is handled separately in
/// [`Decoder::parse_osc`].
pub(super) fn find_string_terminator(data: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if b == 0x9c {
            return Some((i, 1));
        }
        if b == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
            return Some((i, 2));
        }
        i += 1;
    }
    None
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
            return Some(Key::new(c).with_modifiers(KeyModifiers::SHIFT));
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
            return Some(Key::new(c).with_modifiers(KeyModifiers::CTRL));
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
            return Some(Key::new(code).with_modifiers(m));
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
    Key::new(code).with_modifiers(mods)
}

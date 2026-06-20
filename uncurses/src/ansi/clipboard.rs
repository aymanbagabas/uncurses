//! Clipboard and selection access through OSC 52.
//!
//! ## Category
//!
//! OSC 52 sets, requests, or clears named clipboards/selections. This module
//! provides the common system clipboard (`c`) and primary selection (`p`) selector
//! bytes plus writers for each operation.
//!
//! ## OSC framing
//!
//! The emitted sequences use 7-bit OSC with BEL termination:
//!
//! ```text
//! ESC ] 52 ; c ; aGk= BEL
//! ──┬── ┬─   ┬   ─┬─  ─┬─
//!  OSC code  Pc  base64 terminator
//! ```
//!
//! Clipboard data is base64-encoded by [`write_set_clipboard`]. Request and
//! clear operations use `?` or an empty payload in the same field.
//!
//! ## Mode interaction
//!
//! OSC 52 is not gated by an ANSI/DEC mode. Terminals may still reject clipboard
//! access by policy; this module only encodes the request bytes.

use std::io::{self, Write};

/// OSC 52 selector byte `c` for the system clipboard.
pub const SYSTEM_CLIPBOARD: u8 = b'c';

/// OSC 52 selector byte `p` for the primary selection.
pub const PRIMARY_CLIPBOARD: u8 = b'p';

/// Set a clipboard or selection with `ESC ] 52 ; <pc> ; <base64-data> BEL`.
///
/// `pc` is a selector such as [`SYSTEM_CLIPBOARD`] or [`PRIMARY_CLIPBOARD`]. `data` is base64-encoded by this function before it is emitted.
pub fn write_set_clipboard<W: Write>(w: &mut W, pc: u8, data: &[u8]) -> io::Result<()> {
    let encoded = base64_encode(data);
    write!(w, "\x1b]52;{};{}\x07", pc as char, encoded)
}

/// Request clipboard contents with `ESC ] 52 ; <pc> ; ? BEL`.
///
/// `pc` is written as a single selector character. Replies, when allowed by terminal policy, arrive asynchronously as OSC 52 data.
pub fn write_request_clipboard<W: Write>(w: &mut W, pc: u8) -> io::Result<()> {
    write!(w, "\x1b]52;{};?\x07", pc as char)
}

/// Clear a clipboard or selection with `ESC ] 52 ; <pc> ; BEL`.
///
/// The payload field is intentionally empty; `pc` selects which clipboard or selection to clear.
pub fn write_clear_clipboard<W: Write>(w: &mut W, pc: u8) -> io::Result<()> {
    write!(w, "\x1b]52;{};\x07", pc as char)
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | input[i + 2] as u32;
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHABET[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64() {
        assert_eq!(base64_encode(b"hi"), "aGk=");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_set_clipboard() {
        let mut buf = Vec::new();
        write_set_clipboard(&mut buf, SYSTEM_CLIPBOARD, b"hi").unwrap();
        assert_eq!(buf, b"\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn test_request_clipboard() {
        let mut buf = Vec::new();
        write_request_clipboard(&mut buf, PRIMARY_CLIPBOARD).unwrap();
        assert_eq!(buf, b"\x1b]52;p;?\x07");
    }
}

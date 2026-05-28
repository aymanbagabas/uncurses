//! Clipboard manipulation via OSC 52.

use std::io::{self, Write};

/// System clipboard (`c`).
pub const SYSTEM_CLIPBOARD: u8 = b'c';

/// Primary selection (`p`).
pub const PRIMARY_CLIPBOARD: u8 = b'p';

/// Write data to the given clipboard (`OSC 52 ; Pc ; <base64> ST`).
///
/// `data` is base64-encoded by this function.
pub fn write_set_clipboard<W: Write>(w: &mut W, pc: u8, data: &[u8]) -> io::Result<()> {
    let encoded = base64_encode(data);
    write!(w, "\x1b]52;{};{}\x07", pc as char, encoded)
}

/// Request the contents of the given clipboard (`OSC 52 ; Pc ; ? ST`).
pub fn write_request_clipboard<W: Write>(w: &mut W, pc: u8) -> io::Result<()> {
    write!(w, "\x1b]52;{};?\x07", pc as char)
}

/// Clear the given clipboard (`OSC 52 ; Pc ; ST`).
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

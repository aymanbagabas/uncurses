//! Inline graphics encoders for DCS and APC image protocols.
//!
//! ## Category
//!
//! This module frames image payloads as either Sixel DCS strings or APC graphics
//! strings. It does not inspect, compress, or validate the image payload itself.
//!
//! ## String-control framing
//!
//! Both writers use 7-bit string controls terminated by `ST` (`ESC \\`):
//!
//! ```text
//! ESC P ... q payload ESC \\     DCS Sixel
//! ESC _ G opts ; payload ESC \\  APC graphics
//! ──┬──                 ──┬──
//! intro               terminator
//! ```
//!
//! ## Mode interaction
//!
//! Inline graphics are not toggled by a mode in this module. Terminals may impose
//! their own size, capability, or security policy on received payloads.

use std::io::{self, Write};

/// Frame a Sixel payload as `ESC P <p1> ; <p2> [;<p3>] q <payload> ESC \`.
///
/// `p1` and `p2` are omitted when negative, while their semicolon separator remains. `p3` is emitted only when greater than zero. `payload` is copied verbatim between the `q` final byte and `ST`.
pub fn write_sixel<W: Write>(
    w: &mut W,
    p1: i32,
    p2: i32,
    p3: i32,
    payload: &[u8],
) -> io::Result<()> {
    w.write_all(b"\x1bP")?;
    if p1 >= 0 {
        write!(w, "{p1}")?;
    }
    w.write_all(b";")?;
    if p2 >= 0 {
        write!(w, "{p2}")?;
    }
    if p3 > 0 {
        write!(w, ";{p3}")?;
    }
    w.write_all(b"q")?;
    w.write_all(payload)?;
    w.write_all(b"\x1b\\")
}

/// Frame a graphics payload as `ESC _ G <options> [;<payload>] ESC \`.
///
/// `options` are emitted verbatim and joined with commas. The semicolon before `payload` is omitted when `payload` is empty.
pub fn write_kitty_graphics<W: Write>(
    w: &mut W,
    options: &[&str],
    payload: &[u8],
) -> io::Result<()> {
    w.write_all(b"\x1b_G")?;
    for (i, opt) in options.iter().enumerate() {
        if i > 0 {
            w.write_all(b",")?;
        }
        w.write_all(opt.as_bytes())?;
    }
    if !payload.is_empty() {
        w.write_all(b";")?;
        w.write_all(payload)?;
    }
    w.write_all(b"\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sixel_minimal() {
        let mut buf = Vec::new();
        write_sixel(&mut buf, 0, 1, 0, b"#0;2;0;0;0").unwrap();
        assert_eq!(buf, b"\x1bP0;1q#0;2;0;0;0\x1b\\");
    }

    #[test]
    fn test_kitty_graphics() {
        let mut buf = Vec::new();
        write_kitty_graphics(&mut buf, &["a=T", "f=32", "s=10", "v=20"], b"AAAA").unwrap();
        assert_eq!(buf, b"\x1b_Ga=T,f=32,s=10,v=20;AAAA\x1b\\");
    }
}

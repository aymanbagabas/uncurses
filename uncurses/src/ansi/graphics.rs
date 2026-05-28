//! Sixel and Kitty graphics protocols.

use std::io::{self, Write};

/// Encode a Sixel image (`DCS p1 ; p2 ; p3 q <payload> ST`).
///
/// * `p1` — pixel aspect ratio (deprecated; pass `-1` to omit).
/// * `p2` — background-color treatment (0 = transparent; many terminals only
///   render correctly with 1). Pass `-1` to omit.
/// * `p3` — horizontal grid size (rarely used; pass `0` to omit).
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

/// Encode a Kitty graphics image (`APC G opts ; payload ST`).
///
/// `options` is a slice of `"key=value"` strings (the function joins them with
/// `,`). When `payload` is empty, the `;` separator is omitted.
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

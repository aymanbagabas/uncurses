//! Termcap/terminfo capability query (XTGETTCAP, `DCS + q ... ST`).

use std::io::{self, Write};

/// Encode an `XTGETTCAP` request for the given terminfo capability names.
///
/// Each name is hex-encoded (uppercase) and joined with `;`.
pub fn write_xtgettcap<W: Write>(w: &mut W, caps: &[&str]) -> io::Result<()> {
    if caps.is_empty() {
        return Ok(());
    }
    w.write_all(b"\x1bP+q")?;
    for (i, c) in caps.iter().enumerate() {
        if i > 0 {
            w.write_all(b";")?;
        }
        for byte in c.bytes() {
            write!(w, "{byte:02X}")?;
        }
    }
    w.write_all(b"\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xtgettcap() {
        let mut buf = Vec::new();
        write_xtgettcap(&mut buf, &["Co", "TN"]).unwrap();
        assert_eq!(buf, b"\x1bP+q436F;544E\x1b\\");
    }

    #[test]
    fn test_xtgettcap_empty() {
        let mut buf = Vec::new();
        write_xtgettcap(&mut buf, &[]).unwrap();
        assert!(buf.is_empty());
    }
}

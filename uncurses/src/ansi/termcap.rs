//! XTGETTCAP capability queries.
//!
//! ## Category
//!
//! XTGETTCAP asks a terminal to report termcap/terminfo-style capability values
//! using a DCS `+q` request.
//!
//! ## DCS framing
//!
//! The writer emits `ESC P + q <hex-name>[;<hex-name>...] ESC \\`. Capability
//! names are uppercase hexadecimal byte strings; an empty request emits nothing.
//!
//! ## Mode interaction
//!
//! XTGETTCAP is a query, not a mode. Replies arrive asynchronously as DCS
//! strings and must be parsed by input handling code.

use std::io::{self, Write};

/// Request terminal capability values with XTGETTCAP, `ESC P + q <hex-caps> ESC \`.
///
/// Each capability name in `caps` is hex-encoded as uppercase bytes and separated with `;`. An empty slice emits nothing.
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
    fn test_xtgettcap_single_key() {
        // The truecolor-probe path queries one capability per request.
        let mut buf = Vec::new();
        write_xtgettcap(&mut buf, &["RGB"]).unwrap();
        assert_eq!(buf, b"\x1bP+q524742\x1b\\");
        let mut buf = Vec::new();
        write_xtgettcap(&mut buf, &["Tc"]).unwrap();
        assert_eq!(buf, b"\x1bP+q5463\x1b\\");
    }

    #[test]
    fn test_xtgettcap_empty() {
        let mut buf = Vec::new();
        write_xtgettcap(&mut buf, &[]).unwrap();
        assert!(buf.is_empty());
    }
}

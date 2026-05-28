//! Linux console palette manipulation (`OSC P` / `OSC R`).
//!
//! These sequences are specific to the Linux text-mode console.

use std::io::{self, Write};

/// Set a palette entry (`OSC P n rrggbb BEL`).
///
/// `index` must be 0–15; the function is a no-op otherwise.
pub fn write_set_palette<W: Write>(w: &mut W, index: u8, r: u8, g: u8, b: u8) -> io::Result<()> {
    if index > 15 {
        return Ok(());
    }
    write!(w, "\x1b]P{index:x}{r:02x}{g:02x}{b:02x}\x07")
}

/// Reset the palette to defaults (`OSC R BEL`).
pub const RESET_PALETTE: &[u8] = b"\x1b]R\x07";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_palette() {
        let mut buf = Vec::new();
        write_set_palette(&mut buf, 1, 0xff, 0x00, 0x80).unwrap();
        assert_eq!(buf, b"\x1b]P1ff0080\x07");
    }

    #[test]
    fn test_out_of_range() {
        let mut buf = Vec::new();
        write_set_palette(&mut buf, 16, 0, 0, 0).unwrap();
        assert!(buf.is_empty());
    }
}

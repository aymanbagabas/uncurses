//! Linux-console palette sequences.
//!
//! ## Category
//!
//! This module encodes the compact OSC `P`/`R` palette controls used by the Linux
//! text console: setting one of 16 palette entries and resetting all entries.
//!
//! ## OSC framing
//!
//! Palette writes use `ESC ] P n rrggbb BEL`, where `n` is a single hexadecimal
//! palette index. Reset is the fixed byte string `ESC ] R BEL`.
//!
//! ## Mode interaction
//!
//! These sequences do not depend on ANSI or DEC modes and are specific to
//! terminals that implement this palette protocol.

use std::io::{self, Write};

/// Set a 16-color palette entry with `ESC ] P <index-hex> <rrggbb> BEL`.
///
/// `index` must be `0..=15`; values outside that range emit nothing. RGB channels are formatted as two lowercase hexadecimal digits each.
pub fn write_set_palette<W: Write>(w: &mut W, index: u8, r: u8, g: u8, b: u8) -> io::Result<()> {
    if index > 15 {
        return Ok(());
    }
    write!(w, "\x1b]P{index:x}{r:02x}{g:02x}{b:02x}\x07")
}

/// Reset the Linux-console palette: exact bytes `ESC ] R BEL` (`b"\x1b]R\x07"`).
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

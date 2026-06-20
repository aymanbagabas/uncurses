//! In-band terminal resize reports for DEC private mode 2048.
//!
//! ## Category
//!
//! This module encodes a terminal-to-application resize notification as an
//! XTWINOPS-shaped CSI `t` sequence carrying cell and pixel dimensions.
//!
//! ## CSI format
//!
//! The emitted format is `ESC [ 48 ; height_cells ; width_cells ; height_px ; width_px t`.
//! Dimensions are decimal integers and are written exactly as provided.
//!
//! ## Mode interaction
//!
//! Applications request these reports by enabling
//! [`Mode::IN_BAND_RESIZE`](crate::ansi::mode::Mode::IN_BAND_RESIZE), DEC private
//! mode 2048. This module only encodes the report payload.

use std::io::{self, Write};

/// Encode an in-band resize report as `ESC [ 48 ; <height_cells> ; <width_cells> ; <height_pixels> ; <width_pixels> t`.
///
/// The report is intended for applications that enabled [`Mode::IN_BAND_RESIZE`](crate::ansi::mode::Mode::IN_BAND_RESIZE). All dimensions are emitted as decimal `u16` values.
pub fn write_in_band_resize<W: Write>(
    w: &mut W,
    height_cells: u16,
    width_cells: u16,
    height_pixels: u16,
    width_pixels: u16,
) -> io::Result<()> {
    write!(
        w,
        "\x1b[48;{height_cells};{width_cells};{height_pixels};{width_pixels}t",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_band_resize() {
        let mut buf = Vec::new();
        write_in_band_resize(&mut buf, 24, 80, 480, 800).unwrap();
        assert_eq!(buf, b"\x1b[48;24;80;480;800t");
    }
}

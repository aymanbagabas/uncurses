//! In-band terminal resize encoding (mode 2048).
//!
//! See: https://gist.github.com/rockorager/e695fb2924d36b2bcf1fff4a3704bd83

use std::io::{self, Write};

/// Encode an in-band terminal resize event
/// (`CSI 48 ; h_cells ; w_cells ; h_px ; w_px t`).
///
/// Applications enable in-band resize by setting [`crate::ansi::mode::Mode::IN_BAND_RESIZE`].
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

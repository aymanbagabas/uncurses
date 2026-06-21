//! XTWINOPS window-operation requests and reports.
//!
//! ## Category
//!
//! XTWINOPS is the CSI `t` family for window and text-area operations such as
//! resizing and querying pixel or cell dimensions.
//!
//! ## CSI format
//!
//! The generic shape is `ESC [ op [;arg...] t`. Operation numbers are decimal;
//! additional parameters are operation-specific.
//!
//! ## Mode interaction
//!
//! XTWINOPS requests are not enabled by a mode in this module. Related in-band
//! resize notifications are controlled separately by
//! [`Mode::IN_BAND_RESIZE`](crate::ansi::mode::Mode::IN_BAND_RESIZE).

use std::io::{self, Write};

pub mod op {
    //! Common XTWINOPS operation numbers.
    //!
    //! Each value is the first `Ps` parameter in an XTWINOPS `CSI ... t`
    //! sequence. Pass one as the `p` argument to [`write_window_op`](crate::ansi::winop::write_window_op); any
    //! extra `Ps` parameters are operation-specific. These constants only
    //! name window operation numbers; they do not enable terminal modes.
    /// Operation `4`: resize the window in pixels with arguments `height ; width`, yielding `ESC [ 4 ; <height> ; <width> t`.
    pub const RESIZE_WINDOW: u16 = 4;
    /// Operation `14`: request window pixel size with `ESC [ 14 t`; replies use operation `4` with height and width.
    pub const REQUEST_WINDOW_SIZE: u16 = 14;
    /// Operation `16`: request character-cell pixel size with `ESC [ 16 t`; replies use operation `6` with height and width.
    pub const REQUEST_CELL_SIZE: u16 = 16;
    /// Operation `18`: request text-area size in cells with `ESC [ 18 t`; replies use operation `8` with rows and columns.
    pub const REQUEST_TEXT_AREA_SIZE: u16 = 18;
}

/// Request window pixel size: exact bytes `ESC [ 14 t` (`b"\x1b[14t"`).
///
/// A compatible terminal replies with `ESC [ 4 ; <height> ; <width> t`.
pub const REQUEST_WINDOW_PIXEL_SIZE: &[u8] = b"\x1b[14t";

/// Request character-cell pixel size: exact bytes `ESC [ 16 t` (`b"\x1b[16t"`).
///
/// A compatible terminal replies with `ESC [ 6 ; <height> ; <width> t`.
pub const REQUEST_CELL_PIXEL_SIZE: &[u8] = b"\x1b[16t";

/// Encode a generic XTWINOPS sequence.
///
/// `p` is the operation number and `ps` are additional decimal parameters. `p == 0` emits nothing; otherwise the format is `ESC [ <p> [;<ps>...] t`.
pub fn write_window_op<W: Write>(w: &mut W, p: u16, ps: &[u16]) -> io::Result<()> {
    if p == 0 {
        return Ok(());
    }
    w.write_all(b"\x1b[")?;
    write!(w, "{p}")?;
    for v in ps {
        write!(w, ";{v}")?;
    }
    w.write_all(b"t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_cell_size() {
        let mut buf = Vec::new();
        write_window_op(&mut buf, op::REQUEST_CELL_SIZE, &[]).unwrap();
        assert_eq!(buf, b"\x1b[16t");
    }

    #[test]
    fn test_resize() {
        let mut buf = Vec::new();
        write_window_op(&mut buf, op::RESIZE_WINDOW, &[480, 800]).unwrap();
        assert_eq!(buf, b"\x1b[4;480;800t");
    }
}

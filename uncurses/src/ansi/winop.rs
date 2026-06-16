//! XTWINOPS (window operations).
//!
//! `CSI Ps ; Ps ; Ps t` — a multi-purpose set of window-manipulation requests.
//! Common operations include resizing, raising, lowering, reporting size, etc.

use std::io::{self, Write};

/// Common XTWINOPS operation numbers.
pub mod op {
    /// Resize window in pixels (`Ps;height;width t`).
    pub const RESIZE_WINDOW: u16 = 4;
    /// Request size of window in pixels (`14 t` → reply `CSI 4;h;w t`).
    pub const REQUEST_WINDOW_SIZE: u16 = 14;
    /// Request size of cells in pixels (`16 t` → reply `CSI 6;h;w t`).
    pub const REQUEST_CELL_SIZE: u16 = 16;
    /// Request size of window in cells (`18 t` → reply `CSI 8;rows;cols t`).
    pub const REQUEST_TEXT_AREA_SIZE: u16 = 18;
}

/// Request the window pixel size (`CSI 14 t`). Reply: `CSI 4;h;w t`.
pub const REQUEST_WINDOW_PIXEL_SIZE: &[u8] = b"\x1b[14t";

/// Request the character cell pixel size (`CSI 16 t`). Reply: `CSI 6;h;w t`.
pub const REQUEST_CELL_PIXEL_SIZE: &[u8] = b"\x1b[16t";

/// Encode an XTWINOPS sequence (`CSI p[;ps...] t`).
///
/// Returns the empty result if `p == 0`.
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

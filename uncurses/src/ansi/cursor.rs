//! Cursor movement escape sequences.

use std::io::{self, Write};

/// Move cursor to absolute position (CUP). Both row and col are 0-based.
pub fn write_cup<W: Write>(w: &mut W, row: u16, col: u16) -> io::Result<()> {
    if row == 0 && col == 0 {
        w.write_all(b"\x1b[H")
    } else if col == 0 {
        write!(w, "\x1b[{}H", row + 1)
    } else {
        write!(w, "\x1b[{};{}H", row + 1, col + 1)
    }
}

/// Move cursor up `n` rows (CUU).
pub fn write_cuu<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[A"),
        _ => write!(w, "\x1b[{n}A"),
    }
}

/// Move cursor down `n` rows (CUD).
pub fn write_cud<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[B"),
        _ => write!(w, "\x1b[{n}B"),
    }
}

/// Move cursor forward `n` columns (CUF).
pub fn write_cuf<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[C"),
        _ => write!(w, "\x1b[{n}C"),
    }
}

/// Move cursor backward `n` columns (CUB).
pub fn write_cub<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[D"),
        _ => write!(w, "\x1b[{n}D"),
    }
}

/// Move cursor to column `col` (CHA). 0-based.
pub fn write_cha<W: Write>(w: &mut W, col: u16) -> io::Result<()> {
    if col == 0 {
        w.write_all(b"\x1b[G")
    } else {
        write!(w, "\x1b[{}G", col + 1)
    }
}

/// Move cursor to row `row` (VPA). 0-based.
pub fn write_vpa<W: Write>(w: &mut W, row: u16) -> io::Result<()> {
    if row == 0 {
        w.write_all(b"\x1b[d")
    } else {
        write!(w, "\x1b[{}d", row + 1)
    }
}

/// Move cursor to next line (CNL).
pub fn write_cnl<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[E"),
        _ => write!(w, "\x1b[{n}E"),
    }
}

/// Move cursor to previous line (CPL).
pub fn write_cpl<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[F"),
        _ => write!(w, "\x1b[{n}F"),
    }
}

/// Save cursor position (DECSC).
pub fn write_save_cursor<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b7")
}

/// Restore cursor position (DECRC).
pub fn write_restore_cursor<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b8")
}

/// Reverse Index (RI) — move cursor up one line, scrolling if at top.
///
/// Writes the 7-bit form `ESC M`; the 8-bit form is the single byte
/// [`crate::ansi::c1::RI`] (0x8D).
pub fn write_reverse_index<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1bM")
}

/// Index (IND) — move cursor down one line, scrolling if at bottom.
///
/// Writes the 7-bit form `ESC D`; the 8-bit form is the single byte
/// [`crate::ansi::c1::IND`] (0x84).
pub fn write_index<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1bD")
}

/// Horizontal-Vertical Position (HVP, `CSI Pl;Pc f`). Alias of CUP but with
/// final byte `f`. Both row and col are 0-based.
pub fn write_hvp<W: Write>(w: &mut W, row: u16, col: u16) -> io::Result<()> {
    if row == 0 && col == 0 {
        w.write_all(b"\x1b[f")
    } else if col == 0 {
        write!(w, "\x1b[{}f", row + 1)
    } else {
        write!(w, "\x1b[{};{}f", row + 1, col + 1)
    }
}

/// Horizontal Position Absolute (HPA, `CSI Pn ``). 0-based column.
pub fn write_hpa<W: Write>(w: &mut W, col: u16) -> io::Result<()> {
    if col == 0 {
        w.write_all(b"\x1b[`")
    } else {
        write!(w, "\x1b[{}`", col + 1)
    }
}

/// Horizontal Position Relative (HPR, `CSI Pn a`). Move cursor right `n`.
pub fn write_hpr<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[a"),
        _ => write!(w, "\x1b[{n}a"),
    }
}

/// Vertical Position Relative (VPR, `CSI Pn e`). Move cursor down `n`.
pub fn write_vpr<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[e"),
        _ => write!(w, "\x1b[{n}e"),
    }
}

/// Cursor Horizontal forward Tab (CHT, `CSI Pn I`). Move cursor forward
/// `n` tab stops.
pub fn write_cht<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[I"),
        _ => write!(w, "\x1b[{n}I"),
    }
}

/// Save cursor position via the alternate ANSI form (`CSI s`).
///
/// This is the SCO form; [`write_save_cursor`] emits the DEC form `ESC 7`.
pub fn write_save_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[s")
}

/// Restore cursor position via the alternate ANSI form (`CSI u`).
///
/// This is the SCO form; [`write_restore_cursor`] emits the DEC form `ESC 8`.
pub fn write_restore_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[u")
}

/// Set pointer (mouse cursor) shape (OSC 22).
pub fn write_set_pointer_shape<W: Write>(w: &mut W, shape: &str) -> io::Result<()> {
    write!(w, "\x1b]22;{shape}\x1b\\")
}

/// Request extended cursor position report (DECXCPR, `CSI ? 6 n`).
pub fn write_request_extended_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[?6n")
}

/// Cursor style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CursorStyle {
    #[default]
    Default,
    BlinkingBlock,
    SteadyBlock,
    BlinkingUnderline,
    SteadyUnderline,
    BlinkingBar,
    SteadyBar,
}

impl CursorStyle {
    fn param(self) -> u8 {
        match self {
            CursorStyle::Default => 0,
            CursorStyle::BlinkingBlock => 1,
            CursorStyle::SteadyBlock => 2,
            CursorStyle::BlinkingUnderline => 3,
            CursorStyle::SteadyUnderline => 4,
            CursorStyle::BlinkingBar => 5,
            CursorStyle::SteadyBar => 6,
        }
    }
}

/// Set cursor style (DECSCUSR).
pub fn write_cursor_style<W: Write>(w: &mut W, style: CursorStyle) -> io::Result<()> {
    write!(w, "\x1b[{} q", style.param())
}

/// Request cursor position (CPR — DSR 6).
pub fn write_request_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[6n")
}

/// Tab forward.
pub fn write_tab<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\t")
}

/// Cursor backward tab (CBT).
pub fn write_backtab<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[Z"),
        _ => write!(w, "\x1b[{n}Z"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cup_origin() {
        let mut buf = Vec::new();
        write_cup(&mut buf, 0, 0).unwrap();
        assert_eq!(buf, b"\x1b[H");
    }

    #[test]
    fn test_cup_row_only() {
        let mut buf = Vec::new();
        write_cup(&mut buf, 5, 0).unwrap();
        assert_eq!(buf, b"\x1b[6H");
    }

    #[test]
    fn test_cup_both() {
        let mut buf = Vec::new();
        write_cup(&mut buf, 10, 20).unwrap();
        assert_eq!(buf, b"\x1b[11;21H");
    }

    #[test]
    fn test_cuu_single() {
        let mut buf = Vec::new();
        write_cuu(&mut buf, 1).unwrap();
        assert_eq!(buf, b"\x1b[A");
    }

    #[test]
    fn test_cuu_multi() {
        let mut buf = Vec::new();
        write_cuu(&mut buf, 5).unwrap();
        assert_eq!(buf, b"\x1b[5A");
    }

    #[test]
    fn test_cursor_style() {
        let mut buf = Vec::new();
        write_cursor_style(&mut buf, CursorStyle::SteadyBar).unwrap();
        assert_eq!(buf, b"\x1b[6 q");
    }

    #[test]
    fn test_cup_cost() {
        use crate::ansi::cost::cup_cost;
        assert_eq!(cup_cost(0, 0), 3); // \x1b[H
        assert_eq!(cup_cost(9, 0), 5); // \x1b[10H
        assert_eq!(cup_cost(9, 9), 8); // \x1b[10;10H
    }

    // --- DECSCUSR encoding tests ---
    //
    // `CursorStyle` merges shape+blink into a single enum; each variant
    // maps to a stable DECSCUSR parameter emitted by
    // `write_cursor_style`. The mapping is exercised by writing each
    // variant and asserting the emitted byte stream matches
    // `CSI <param> SP q`.

    #[test]
    fn cursor_style_decscusr_encoding() {
        for (style, param) in [
            (CursorStyle::BlinkingBlock, 1),
            (CursorStyle::SteadyBlock, 2),
            (CursorStyle::BlinkingUnderline, 3),
            (CursorStyle::SteadyUnderline, 4),
            (CursorStyle::BlinkingBar, 5),
            (CursorStyle::SteadyBar, 6),
        ] {
            let mut buf: Vec<u8> = Vec::new();
            write_cursor_style(&mut buf, style).unwrap();
            let want = format!("\x1b[{param} q");
            assert_eq!(String::from_utf8(buf).unwrap(), want, "style {style:?}");
        }
    }
}

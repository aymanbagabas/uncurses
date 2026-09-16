//! Screen, line, scroll-region, and tab-stop manipulation.
//!
//! ## Category
//!
//! This module emits CSI controls for erasing, inserting/deleting characters and
//! lines, scrolling, setting vertical and horizontal margins, and tab-stop
//! management.
//!
//! ## CSI conventions
//!
//! Counted operations use the terminal default parameter where possible: for
//! example `n == 1` emits `ESC [ X` for ECH and `ESC [ L` for IL, and `n == 0`
//! emits nothing at all. Erase
//! operations use parameter `0` as the omitted default.
//!
//! ## Mode interaction
//!
//! Left/right margins are interpreted as DECSLRM when
//! [`Mode::LEFT_RIGHT_MARGIN`](crate::ansi::mode::Mode::LEFT_RIGHT_MARGIN) is
//! enabled. Top/bottom scroll margins are set by DECSTBM and affect absolute
//! cursor movement when origin mode is active.

use std::io::{self, Write};

/// Erase `n` character cells at the cursor with ECH.
///
/// `n == 0` emits nothing, since an absent parameter means one rather than
/// none; `n == 1` emits `ESC [ X`; larger counts emit `ESC [ <n> X`. Erased cells are replaced with blank cells using the active rendition.
pub fn write_ech<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[X"),
        _ => write!(w, "\x1b[{n}X"),
    }
}

/// Repeat the preceding printable character with REP.
///
/// `n == 0` emits nothing, since an absent parameter means one rather than
/// none; `n == 1` emits `ESC [ b`; larger counts emit `ESC [ <n> b`. Use only when the terminal supports REP and the preceding character is repeatable.
pub fn write_rep<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[b"),
        _ => write!(w, "\x1b[{n}b"),
    }
}

/// Insert `n` blank character cells at the cursor with ICH.
///
/// `n == 0` emits nothing, since an absent parameter means one rather than
/// none; `n == 1` emits `ESC [ @`; larger counts emit `ESC [ <n> @`. Existing cells shift right within the line.
pub fn write_ich<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[@"),
        _ => write!(w, "\x1b[{n}@"),
    }
}

/// Delete `n` character cells at the cursor with DCH.
///
/// `n == 0` emits nothing, since an absent parameter means one rather than
/// none; `n == 1` emits `ESC [ P`; larger counts emit `ESC [ <n> P`. Cells to the right shift left and blanks are inserted at the right edge.
pub fn write_dch<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[P"),
        _ => write!(w, "\x1b[{n}P"),
    }
}

/// Erase in line with EL, `ESC [ <n> K`.
///
/// `n == 0` emits `ESC [ K` (cursor through end of line), `1` erases start through cursor, and `2` erases the entire line. Other values are emitted as provided.
pub fn write_el<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => w.write_all(b"\x1b[K"),
        _ => write!(w, "\x1b[{n}K"),
    }
}

/// Erase in display with ED, `ESC [ <n> J`.
///
/// `n == 0` emits `ESC [ J` (cursor through end of screen), `1` erases start through cursor, `2` erases the visible screen, and `3` requests scrollback/display clearing where supported.
pub fn write_ed<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => w.write_all(b"\x1b[J"),
        _ => write!(w, "\x1b[{n}J"),
    }
}

/// Erase from cursor through end of line: exact bytes `ESC [ K` (`EL 0`).
pub const ERASE_LINE_RIGHT: &[u8] = b"\x1b[K";
/// Erase from start of line through cursor: exact bytes `ESC [ 1 K` (`EL 1`).
pub const ERASE_LINE_LEFT: &[u8] = b"\x1b[1K";
/// Erase the entire current line: exact bytes `ESC [ 2 K` (`EL 2`).
pub const ERASE_ENTIRE_LINE: &[u8] = b"\x1b[2K";
/// Erase from cursor through end of screen: exact bytes `ESC [ J` (`ED 0`).
pub const ERASE_SCREEN_BELOW: &[u8] = b"\x1b[J";
/// Erase from start of screen through cursor: exact bytes `ESC [ 1 J` (`ED 1`).
pub const ERASE_SCREEN_ABOVE: &[u8] = b"\x1b[1J";
/// Erase the visible screen: exact bytes `ESC [ 2 J` (`ED 2`).
pub const ERASE_ENTIRE_SCREEN: &[u8] = b"\x1b[2J";
/// Erase the display including scrollback where supported: exact bytes `ESC [ 3 J` (`ED 3`).
pub const ERASE_ENTIRE_DISPLAY: &[u8] = b"\x1b[3J";

/// Write [`ERASE_LINE_RIGHT`], `ESC [ K`, erasing from cursor through end of line.
pub fn write_erase_to_eol<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(ERASE_LINE_RIGHT)
}

/// Write [`ERASE_ENTIRE_LINE`], `ESC [ 2 K`, erasing the entire current line.
pub fn write_erase_line<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(ERASE_ENTIRE_LINE)
}

/// Write [`ERASE_SCREEN_BELOW`], `ESC [ J`, erasing from cursor through end of screen.
pub fn write_erase_below<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(ERASE_SCREEN_BELOW)
}

/// Write [`ERASE_ENTIRE_SCREEN`], `ESC [ 2 J`, erasing the visible screen.
pub fn write_erase_screen<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(ERASE_ENTIRE_SCREEN)
}

/// Insert `n` blank lines with IL.
///
/// `n == 0` emits nothing, since an absent parameter means one rather than
/// none; `n == 1` emits `ESC [ L`; larger counts emit `ESC [ <n> L`. Lines below shift downward within the scrolling region.
pub fn write_insert_lines<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[L"),
        _ => write!(w, "\x1b[{n}L"),
    }
}

/// Delete `n` lines with DL.
///
/// `n == 0` emits nothing, since an absent parameter means one rather than
/// none; `n == 1` emits `ESC [ M`; larger counts emit `ESC [ <n> M`. Lines below shift upward within the scrolling region.
pub fn write_delete_lines<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[M"),
        _ => write!(w, "\x1b[{n}M"),
    }
}

/// Scroll up by `n` lines with SU.
///
/// `n == 0` emits nothing, since an absent parameter means one rather than
/// none; `n == 1` emits `ESC [ S`; larger counts emit `ESC [ <n> S`.
pub fn write_scroll_up<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[S"),
        _ => write!(w, "\x1b[{n}S"),
    }
}

/// Scroll down by `n` lines with SD.
///
/// `n == 0` emits nothing, since an absent parameter means one rather than
/// none; `n == 1` emits `ESC [ T`; larger counts emit `ESC [ <n> T`.
pub fn write_scroll_down<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => Ok(()),
        1 => w.write_all(b"\x1b[T"),
        _ => write!(w, "\x1b[{n}T"),
    }
}

/// Set top and bottom margins with DECSTBM, `ESC [ <top+1> ; <bottom+1> r`.
///
/// `top` and `bottom` are zero-based API row indices; the terminal parameters are one-based.
pub fn write_scroll_region<W: Write>(w: &mut W, top: u16, bottom: u16) -> io::Result<()> {
    write!(w, "\x1b[{};{}r", top + 1, bottom + 1)
}

/// Reset top/bottom margins to the full screen with exact bytes `ESC [ r`.
pub fn write_reset_scroll_region<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[r")
}

/// Set left and right margins with DECSLRM, `ESC [ <left> ; <right> s`.
///
/// Arguments are emitted as one-based terminal parameters where `0` means omitted/default. When both are `0`, this emits `ESC [ s`, the same byte sequence as the alternate save-cursor form outside DECSLRM context.
pub fn write_set_left_right_margins<W: Write>(w: &mut W, left: u16, right: u16) -> io::Result<()> {
    match (left, right) {
        (0, 0) => w.write_all(b"\x1b[s"),
        (l, 0) => write!(w, "\x1b[{l}s"),
        (0, r) => write!(w, "\x1b[;{r}s"),
        (l, r) => write!(w, "\x1b[{l};{r}s"),
    }
}

/// Set tab stops every eight columns: exact bytes `ESC [ ? 5 W` (DECST8C).
pub const SET_TAB_EVERY_8_COLUMNS: &[u8] = b"\x1b[?5W";

/// Set a horizontal tab stop at the current column: exact bytes `ESC H` (HTS 7-bit form).
pub const HORIZONTAL_TAB_SET: &[u8] = b"\x1bH";

/// Write [`HORIZONTAL_TAB_SET`], `ESC H`, to set a tab stop at the current cursor column.
pub fn write_hts<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(HORIZONTAL_TAB_SET)
}

/// Clear tab stops with TBC, `ESC [ <n> g`.
///
/// `n == 0` emits `ESC [ g` and clears the current-column tab stop. `n == 3` clears all tab stops; other values are emitted as provided.
pub fn write_tbc<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => w.write_all(b"\x1b[g"),
        _ => write!(w, "\x1b[{n}g"),
    }
}

/// Reset tab stops to one every eight columns without DECST8C.
///
/// Clears every existing stop with TBC and re-establishes one at each
/// eighth column through `width` using HTS. The cursor is parked at
/// column zero with CR before and after, and nothing is printed, so the
/// visible row is left untouched. This is the portable fallback for
/// terminals that do not implement [`SET_TAB_EVERY_8_COLUMNS`].
///
/// `width` is the managed width in cells; a `width` of eight or fewer
/// only clears stops, since the first default stop already lies at or
/// past the right edge.
pub fn write_reset_tab_stops_every_8<W: Write>(w: &mut W, width: u16) -> io::Result<()> {
    // Snap to a known reference column before clearing stops.
    w.write_all(b"\r")?;
    write_tbc(w, 3)?;
    let mut col = 8u16;
    let mut moved = false;
    while col < width {
        super::cursor::write_cuf(w, 8)?;
        write_hts(w)?;
        moved = true;
        col += 8;
    }
    // Return to column zero so the inline cursor is left where it started.
    if moved {
        w.write_all(b"\r")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_erase_to_eol() {
        let mut buf = Vec::new();
        write_erase_to_eol(&mut buf).unwrap();
        assert_eq!(buf, b"\x1b[K");
    }

    #[test]
    fn test_scroll_region() {
        let mut buf = Vec::new();
        write_scroll_region(&mut buf, 5, 20).unwrap();
        assert_eq!(buf, b"\x1b[6;21r");
    }

    #[test]
    fn test_el_variants() {
        let mut buf = Vec::new();
        write_el(&mut buf, 0).unwrap();
        write_el(&mut buf, 1).unwrap();
        write_el(&mut buf, 2).unwrap();
        assert_eq!(buf, b"\x1b[K\x1b[1K\x1b[2K");
    }

    #[test]
    fn test_ed_variants() {
        let mut buf = Vec::new();
        write_ed(&mut buf, 0).unwrap();
        write_ed(&mut buf, 1).unwrap();
        write_ed(&mut buf, 2).unwrap();
        write_ed(&mut buf, 3).unwrap();
        assert_eq!(buf, b"\x1b[J\x1b[1J\x1b[2J\x1b[3J");
    }

    #[test]
    fn test_reset_tab_stops_every_8_clears_then_sets() {
        let mut buf = Vec::new();
        write_reset_tab_stops_every_8(&mut buf, 20).unwrap();
        // CR, TBC clear-all, then CUF 8 + HTS for columns 8 and 16, then CR.
        assert_eq!(buf, b"\r\x1b[3g\x1b[8C\x1bH\x1b[8C\x1bH\r");
    }

    #[test]
    fn test_reset_tab_stops_every_8_narrow_only_clears() {
        // Width 8 or less has no interior stop, so nothing moves the cursor
        // and only the clear is emitted (no trailing CR).
        let mut buf = Vec::new();
        write_reset_tab_stops_every_8(&mut buf, 8).unwrap();
        assert_eq!(buf, b"\r\x1b[3g");
    }

    #[test]
    fn test_decslrm() {
        let mut buf = Vec::new();
        write_set_left_right_margins(&mut buf, 5, 70).unwrap();
        assert_eq!(buf, b"\x1b[5;70s");
    }

    #[test]
    fn test_tab_stops() {
        let mut buf = Vec::new();
        write_hts(&mut buf).unwrap();
        write_tbc(&mut buf, 0).unwrap();
        write_tbc(&mut buf, 3).unwrap();
        assert_eq!(buf, b"\x1bH\x1b[g\x1b[3g");
    }
}

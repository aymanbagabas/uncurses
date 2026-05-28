//! Screen manipulation sequences: erase, insert/delete, scroll.

use std::io::{self, Write};

/// Erase `n` characters at cursor (ECH).
pub fn write_ech<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    if n <= 1 {
        w.write_all(b"\x1b[X")
    } else {
        write!(w, "\x1b[{n}X")
    }
}

/// Repeat preceding character `n` times (REP).
pub fn write_rep<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    if n <= 1 {
        w.write_all(b"\x1b[b")
    } else {
        write!(w, "\x1b[{n}b")
    }
}

/// Insert `n` blank characters at cursor (ICH).
pub fn write_ich<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    if n <= 1 {
        w.write_all(b"\x1b[@")
    } else {
        write!(w, "\x1b[{n}@")
    }
}

/// Delete `n` characters at cursor (DCH).
pub fn write_dch<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    if n <= 1 {
        w.write_all(b"\x1b[P")
    } else {
        write!(w, "\x1b[{n}P")
    }
}

/// Erase line (EL). `n`:
/// * 0 — from cursor to end of line.
/// * 1 — from start of line to cursor.
/// * 2 — entire line.
pub fn write_el<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => w.write_all(b"\x1b[K"),
        _ => write!(w, "\x1b[{n}K"),
    }
}

/// Erase display (ED). `n`:
/// * 0 — from cursor to end of screen.
/// * 1 — from start of screen to cursor.
/// * 2 — entire screen.
/// * 3 — entire display including scrollback (xterm).
pub fn write_ed<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => w.write_all(b"\x1b[J"),
        _ => write!(w, "\x1b[{n}J"),
    }
}

/// Erase from cursor to end of line (EL 0).
pub const ERASE_LINE_RIGHT: &[u8] = b"\x1b[K";
/// Erase from start of line to cursor (EL 1).
pub const ERASE_LINE_LEFT: &[u8] = b"\x1b[1K";
/// Erase entire line (EL 2).
pub const ERASE_ENTIRE_LINE: &[u8] = b"\x1b[2K";
/// Erase from cursor to end of screen (ED 0).
pub const ERASE_SCREEN_BELOW: &[u8] = b"\x1b[J";
/// Erase from start of screen to cursor (ED 1).
pub const ERASE_SCREEN_ABOVE: &[u8] = b"\x1b[1J";
/// Erase entire screen (ED 2).
pub const ERASE_ENTIRE_SCREEN: &[u8] = b"\x1b[2J";
/// Erase entire display including scrollback (ED 3).
pub const ERASE_ENTIRE_DISPLAY: &[u8] = b"\x1b[3J";

/// Erase from cursor to end of line (EL 0).
pub fn write_erase_to_eol<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(ERASE_LINE_RIGHT)
}

/// Erase entire line (EL 2).
pub fn write_erase_line<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(ERASE_ENTIRE_LINE)
}

/// Erase from cursor to end of screen (ED 0).
pub fn write_erase_below<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(ERASE_SCREEN_BELOW)
}

/// Erase entire screen (ED 2).
pub fn write_erase_screen<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(ERASE_ENTIRE_SCREEN)
}

/// Insert `n` lines at cursor (IL).
pub fn write_insert_lines<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    if n <= 1 {
        w.write_all(b"\x1b[L")
    } else {
        write!(w, "\x1b[{n}L")
    }
}

/// Delete `n` lines at cursor (DL).
pub fn write_delete_lines<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    if n <= 1 {
        w.write_all(b"\x1b[M")
    } else {
        write!(w, "\x1b[{n}M")
    }
}

/// Scroll up `n` lines (SU).
pub fn write_scroll_up<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    if n <= 1 {
        w.write_all(b"\x1b[S")
    } else {
        write!(w, "\x1b[{n}S")
    }
}

/// Scroll down `n` lines (SD).
pub fn write_scroll_down<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    if n <= 1 {
        w.write_all(b"\x1b[T")
    } else {
        write!(w, "\x1b[{n}T")
    }
}

/// Set scroll region (DECSTBM). `top`/`bottom` are zero-based row indices.
pub fn write_scroll_region<W: Write>(w: &mut W, top: u16, bottom: u16) -> io::Result<()> {
    write!(w, "\x1b[{};{}r", top + 1, bottom + 1)
}

/// Reset scroll region to full screen.
pub fn write_reset_scroll_region<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(b"\x1b[r")
}

/// Set left/right margins (DECSLRM). 1-based, 0 means default.
pub fn write_set_left_right_margins<W: Write>(w: &mut W, left: u16, right: u16) -> io::Result<()> {
    match (left, right) {
        (0, 0) => w.write_all(b"\x1b[s"),
        (l, 0) => write!(w, "\x1b[{l}s"),
        (0, r) => write!(w, "\x1b[;{r}s"),
        (l, r) => write!(w, "\x1b[{l};{r}s"),
    }
}

/// Set tab stops every 8 columns (DECST8C, `CSI ? 5 W`).
pub const SET_TAB_EVERY_8_COLUMNS: &[u8] = b"\x1b[?5W";

/// Horizontal Tab Set (HTS, `ESC H`) — set a tab stop at the cursor column.
pub const HORIZONTAL_TAB_SET: &[u8] = b"\x1bH";

/// Set a tab stop at the current cursor column (HTS).
pub fn write_hts<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(HORIZONTAL_TAB_SET)
}

/// Clear tab stops (TBC). `n`:
/// * 0 — clear tab at current column.
/// * 3 — clear all tab stops.
pub fn write_tbc<W: Write>(w: &mut W, n: u16) -> io::Result<()> {
    match n {
        0 => w.write_all(b"\x1b[g"),
        _ => write!(w, "\x1b[{n}g"),
    }
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

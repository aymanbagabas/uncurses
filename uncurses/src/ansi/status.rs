//! Device Status Reports (DSR) and Cursor Position Reports.

use std::io::{self, Write};

/// Request cursor position report (CPR, `CSI 6 n`).
///
/// The terminal responds with `CSI Pl;Pc R`.
pub const REQUEST_CURSOR_POSITION: &[u8] = b"\x1b[6n";

/// Request extended cursor position report (DECXCPR, `CSI ? 6 n`).
///
/// The terminal responds with `CSI ? Pl;Pc[;Pp] R`.
pub const REQUEST_EXTENDED_CURSOR_POSITION: &[u8] = b"\x1b[?6n";

/// Request the terminal's operating-system light/dark color preference
/// (`CSI ? 996 n`).
pub const REQUEST_LIGHT_DARK_REPORT: &[u8] = b"\x1b[?996n";

/// Write the cursor position request (`CSI 6 n`).
pub fn write_request_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_CURSOR_POSITION)
}

/// Write the extended cursor position request (`CSI ? 6 n`).
pub fn write_request_extended_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_EXTENDED_CURSOR_POSITION)
}

/// Write the light or dark preference request (`CSI ? 996 n`).
pub fn write_request_light_dark_report<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_LIGHT_DARK_REPORT)
}

/// Encode a Device Status Report request (DSR).
///
/// * If `dec` is `true`, emits a DEC-style request (`CSI ? Ps n`).
/// * Otherwise emits the ANSI form (`CSI Ps n`).
pub fn write_dsr_request<W: Write>(w: &mut W, dec: bool, ps: u16) -> io::Result<()> {
    if dec {
        write!(w, "\x1b[?{ps}n")
    } else {
        write!(w, "\x1b[{ps}n")
    }
}

/// Encode a Cursor Position Report response (CPR, `CSI Pl;Pc R`).
/// Both `line` and `column` are 1-based.
pub fn write_cpr<W: Write>(w: &mut W, line: u16, column: u16) -> io::Result<()> {
    let l = line.max(1);
    let c = column.max(1);
    write!(w, "\x1b[{l};{c}R")
}

/// Encode an extended Cursor Position Report response (DECXCPR,
/// `CSI ? Pl;Pc[;Pp] R`). `page` of 0 omits the page parameter.
pub fn write_decxcpr<W: Write>(w: &mut W, line: u16, column: u16, page: u16) -> io::Result<()> {
    let l = line.max(1);
    let c = column.max(1);
    if page == 0 {
        write!(w, "\x1b[?{l};{c}R")
    } else {
        write!(w, "\x1b[?{l};{c};{page}R")
    }
}

/// Encode a light/dark mode report (`CSI ? 997 ; 1 n` for dark,
/// `CSI ? 997 ; 2 n` for light).
pub fn write_light_dark_report<W: Write>(w: &mut W, dark: bool) -> io::Result<()> {
    if dark {
        w.write_all(b"\x1b[?997;1n")
    } else {
        w.write_all(b"\x1b[?997;2n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpr() {
        let mut buf = Vec::new();
        write_cpr(&mut buf, 10, 20).unwrap();
        assert_eq!(buf, b"\x1b[10;20R");
    }

    #[test]
    fn test_decxcpr() {
        let mut buf = Vec::new();
        write_decxcpr(&mut buf, 5, 6, 0).unwrap();
        write_decxcpr(&mut buf, 5, 6, 2).unwrap();
        assert_eq!(buf, b"\x1b[?5;6R\x1b[?5;6;2R");
    }

    #[test]
    fn test_dsr_request() {
        let mut buf = Vec::new();
        write_dsr_request(&mut buf, false, 5).unwrap();
        write_dsr_request(&mut buf, true, 996).unwrap();
        assert_eq!(buf, b"\x1b[5n\x1b[?996n");
    }
}

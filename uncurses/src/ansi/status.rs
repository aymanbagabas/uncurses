//! Device Status Reports and cursor-position reports.
//!
//! ## Category
//!
//! This module emits DSR requests and report responses: cursor position, extended
//! cursor position, and light/dark preference reporting. It also carries
//! DECRQSS and its DECRPSS response, which ask after a current setting
//! rather than device status.
//!
//! ## CSI conventions
//!
//! ANSI DSR uses `ESC [ Ps n`; DEC-private DSR inserts `?`. Cursor reports use
//! final byte `R`, while light/dark reports use private DSR numbers.
//!
//! ## Mode interaction
//!
//! The light/dark notification request is related to
//! [`Mode::LIGHT_DARK`](crate::ansi::mode::Mode::LIGHT_DARK), DEC private mode
//! 2031. Cursor-position reports are independent of modes but may be interpreted
//! relative to terminal origin behavior.

use std::io::{self, Write};

/// Request standard cursor position: exact bytes `ESC [ 6 n` (`b"\x1b[6n"`).
///
/// The terminal replies with CPR, `ESC [ <line> ; <column> R`, using one-based coordinates.
pub const REQUEST_CURSOR_POSITION: &[u8] = b"\x1b[6n";

/// Request extended cursor position: exact bytes `ESC [ ? 6 n` (`b"\x1b[?6n"`).
///
/// The terminal replies with a private cursor-position report, optionally including page.
pub const REQUEST_EXTENDED_CURSOR_POSITION: &[u8] = b"\x1b[?6n";

/// Request light/dark preference report: exact bytes `ESC [ ? 996 n` (`b"\x1b[?996n"`).
pub const REQUEST_LIGHT_DARK_REPORT: &[u8] = b"\x1b[?996n";

/// Write [`REQUEST_CURSOR_POSITION`], the standard DSR 6 cursor-position request.
pub fn write_request_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_CURSOR_POSITION)
}

/// Write [`REQUEST_EXTENDED_CURSOR_POSITION`], the DEC private extended cursor-position request.
pub fn write_request_extended_cursor_position<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_EXTENDED_CURSOR_POSITION)
}

/// Write [`REQUEST_LIGHT_DARK_REPORT`], the light/dark preference query.
pub fn write_request_light_dark_report<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_LIGHT_DARK_REPORT)
}

/// Request a current setting with DECRQSS, `ESC P $ q <selector> ESC \`.
///
/// `selector` spells the control function being asked about, as its private
/// prefix, intermediates and final byte: `"m"` for SGR, `" q"` for
/// `DECSCUSR` (cursor style), `"r"` for `DECSTBM` (scrolling region),
/// `"\"q"` for `DECSCA`, `"$|"` for `DECSCPP`. xterm's private requests take
/// a parameter too, as in `">4m"` for `XTQMODKEYS`. An empty selector emits
/// nothing.
///
/// The terminal answers with `ESC P 1 $ r <value><selector> ESC \` when it
/// recognizes the request and `ESC P 0 $ r ESC \` when it does not, decoded
/// as [`Event::SettingReport`](crate::event::Event::SettingReport). Because a
/// refusal echoes nothing back, only the request says which setting it
/// refused.
pub fn write_decrqss<W: Write>(w: &mut W, selector: &str) -> io::Result<()> {
    if selector.is_empty() {
        return Ok(());
    }
    write!(w, "\x1bP$q{selector}\x1b\\")
}

/// Encode a Device Status Report request.
///
/// When `dec` is `false`, the format is `ESC [ <ps> n`; when `dec` is `true`, the format is `ESC [ ? <ps> n`.
pub fn write_dsr_request<W: Write>(w: &mut W, dec: bool, ps: u16) -> io::Result<()> {
    if dec {
        write!(w, "\x1b[?{ps}n")
    } else {
        write!(w, "\x1b[{ps}n")
    }
}

/// Encode a standard Cursor Position Report response, `ESC [ <line> ; <column> R`.
///
/// `line` and `column` are one-based terminal coordinates; values less than `1` are clamped to `1`.
pub fn write_cpr<W: Write>(w: &mut W, line: u16, column: u16) -> io::Result<()> {
    let l = line.max(1);
    let c = column.max(1);
    write!(w, "\x1b[{l};{c}R")
}

/// Encode an extended Cursor Position Report response.
///
/// With `page == 0`, emits `ESC [ ? <line> ; <column> R`; otherwise emits `ESC [ ? <line> ; <column> ; <page> R`. `line` and `column` are clamped to at least `1`.
pub fn write_decxcpr<W: Write>(w: &mut W, line: u16, column: u16, page: u16) -> io::Result<()> {
    let l = line.max(1);
    let c = column.max(1);
    if page == 0 {
        write!(w, "\x1b[?{l};{c}R")
    } else {
        write!(w, "\x1b[?{l};{c};{page}R")
    }
}

/// Encode a DECRQSS response (DECRPSS), `ESC P <ps> $ r <value><selector> ESC \`.
///
/// `ps` reports whether the request was valid. Only two values are defined:
/// `1` for a valid request, which the value and selector then answer, and
/// `0` for an invalid one, which carries no data at all, so pass an empty
/// `value` and `selector` with it. Anything else is written out as given,
/// the same way [`write_dsr_request`] does not judge its own parameter.
///
/// Beware that the VT510 manual documents `0` and `1` the other way around.
/// It is wrong: a VT420 tested in 1996 had them reversed, and vttest, DEC
/// STD 070 and xterm all treat `1` as the valid one.
///
/// The value goes after any private prefix rather than before the whole
/// selector, since the reply spells out the CSI string for the setting:
/// reporting `"4;2"` for `">m"` emits `> 4 ; 2 m`, matching xterm's
/// `XTQMODKEYS`. This is the inverse of the split
/// [`Event::SettingReport`](crate::event::Event::SettingReport) reports.
pub fn write_decrpss<W: Write>(w: &mut W, ps: u16, value: &str, selector: &str) -> io::Result<()> {
    let head = usize::from(selector.starts_with(['<', '=', '>', '?']));
    let (prefix, tail) = selector.split_at(head);
    write!(w, "\x1bP{ps}$r{prefix}{value}{tail}\x1b\\")
}

/// Encode a light/dark report response.
///
/// `dark == true` emits `ESC [ ? 997 ; 1 n`; `false` emits `ESC [ ? 997 ; 2 n`.
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
    fn test_decrqss() {
        let mut buf = Vec::new();
        write_decrqss(&mut buf, "m").unwrap();
        write_decrqss(&mut buf, " q").unwrap();
        write_decrqss(&mut buf, "").unwrap();
        assert_eq!(buf, b"\x1bP$qm\x1b\\\x1bP$q q\x1b\\");
    }

    #[test]
    fn test_decrpss() {
        let mut buf = Vec::new();
        write_decrpss(&mut buf, 1, "0;1", "m").unwrap();
        write_decrpss(&mut buf, 1, "2", " q").unwrap();
        // The value goes after the private prefix, not before it.
        write_decrpss(&mut buf, 1, "4;2", ">m").unwrap();
        // A refusal carries no data.
        write_decrpss(&mut buf, 0, "", "").unwrap();
        assert_eq!(
            buf,
            b"\x1bP1$r0;1m\x1b\\\x1bP1$r2 q\x1b\\\x1bP1$r>4;2m\x1b\\\x1bP0$r\x1b\\"
        );
    }

    #[test]
    fn test_dsr_request() {
        let mut buf = Vec::new();
        write_dsr_request(&mut buf, false, 5).unwrap();
        write_dsr_request(&mut buf, true, 996).unwrap();
        assert_eq!(buf, b"\x1b[5n\x1b[?996n");
    }
}

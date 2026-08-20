//! Device Status Reports and cursor-position reports.
//!
//! ## Category
//!
//! This module emits DSR requests and report responses: cursor position, extended
//! cursor position, light/dark preference, and terminal visibility reporting. It
//! also carries DECRQSS and its DECRPSS response, which ask after a current
//! setting rather than device status.
//!
//! ## CSI conventions
//!
//! ANSI DSR uses `ESC [ Ps n`; DEC-private DSR inserts `?`. Cursor reports use
//! final byte `R`, while light/dark and visibility reports use private DSR
//! numbers.
//!
//! ## Mode interaction
//!
//! The light/dark notification request is related to
//! [`Mode::LIGHT_DARK`](crate::ansi::mode::Mode::LIGHT_DARK), DEC private mode
//! 2031, and the visibility request to
//! [`Mode::VISIBILITY_REPORTS`](crate::ansi::mode::Mode::VISIBILITY_REPORTS),
//! DEC private mode 2033. Both queries report once without changing their
//! mode. Cursor-position reports are independent of modes but may be
//! interpreted relative to terminal origin behavior.

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

/// Request a terminal visibility report: exact bytes `ESC [ ? 998 n` (`b"\x1b[?998n"`).
///
/// The terminal replies with `ESC [ ? 999 ; Ps n`. This query does not change
/// [`Mode::VISIBILITY_REPORTS`](crate::ansi::mode::Mode::VISIBILITY_REPORTS).
pub const REQUEST_VISIBILITY_REPORT: &[u8] = b"\x1b[?998n";

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

/// Write [`REQUEST_VISIBILITY_REPORT`], the one-shot terminal visibility query.
pub fn write_request_visibility_report<W: Write>(w: &mut W) -> io::Result<()> {
    w.write_all(REQUEST_VISIBILITY_REPORT)
}

/// Request a current setting with DECRQSS, `ESC P $ q <selector> ESC \`.
///
/// `selector` spells the control function being asked about, as its private
/// prefix, intermediates and final byte: `"m"` for SGR, `" q"` for
/// `DECSCUSR` (cursor style), `"r"` for `DECSTBM` (scrolling region),
/// `"\"q"` for `DECSCA`, `"$|"` for `DECSCPP`. xterm's private requests take
/// a parameter too, as in `">4m"` for `XTQMODKEYS`. An empty selector is
/// written like any other and the terminal reports it as unrecognized, which
/// keeps one request paired with one reply.
///
/// The terminal answers with DECRPSS: `ESC P 1 $ r <D...D> ESC \` when it
/// recognizes the request, where the data string is the setting spelled out
/// as a CSI sequence without its introducer, and `ESC P 0 $ r ESC \` when it
/// does not. Both decode as
/// [`Event::SettingReport`](crate::event::Event::SettingReport). Because the
/// unrecognized form echoes nothing back, only the request says which setting
/// it was about. Use [`write_decrpss`] to encode either reply.
pub fn write_decrqss<W: Write>(w: &mut W, selector: &str) -> io::Result<()> {
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

/// Encode a DECRPSS report, `ESC P <ps> $ r <D...D> ESC \`.
///
/// This is the terminal's answer to [`write_decrqss`]. `valid` says whether
/// the terminal recognized the request. `settings` is the control function
/// being reported, spelled out as every character of its CSI sequence except
/// the introducer: `"0;4;5;7m"` for SGR, `"1;24r"` for `DECSTBM`, `"2 q"` for
/// `DECSCUSR`, `">4;2m"` for xterm's `XTQMODKEYS`. A terminal sends no data
/// string at all for an invalid request, so `false` emits `ESC P 0 $ r ESC \`
/// and ignores `settings`.
///
/// Beware that the VT510 manual documents `Ps` the other way around, as `0`
/// for valid and `1` for invalid. It is wrong: a VT420 tested in 1996 had the
/// two reversed, and vttest, DEC STD 070 and xterm all treat `1` as the valid
/// one.
pub fn write_decrpss<W: Write>(w: &mut W, valid: bool, settings: &str) -> io::Result<()> {
    if !valid {
        return w.write_all(b"\x1bP0$r\x1b\\");
    }
    write!(w, "\x1bP1$r{settings}\x1b\\")
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

/// Encode a terminal visibility report response.
///
/// `visible == true` emits `ESC [ ? 999 ; 1 n` (potentially visible); `false`
/// emits `ESC [ ? 999 ; 2 n` (not visible).
pub fn write_visibility_report<W: Write>(w: &mut W, visible: bool) -> io::Result<()> {
    if visible {
        w.write_all(b"\x1b[?999;1n")
    } else {
        w.write_all(b"\x1b[?999;2n")
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
        // Replies carry nothing that names the request, so they are matched by
        // order. An empty selector still goes out, so the counts stay level.
        write_decrqss(&mut buf, "").unwrap();
        assert_eq!(buf, b"\x1bP$qm\x1b\\\x1bP$q q\x1b\\\x1bP$q\x1b\\");
    }

    #[test]
    fn test_decrpss() {
        let mut buf = Vec::new();
        // The two examples the VT510 manual gives for DECRPSS.
        write_decrpss(&mut buf, true, "0;4;5;7m").unwrap();
        write_decrpss(&mut buf, true, "1;24r").unwrap();
        // Not from the manual: xterm's private XTQMODKEYS, here to prove a
        // private prefix survives the encoder.
        write_decrpss(&mut buf, true, ">4;2m").unwrap();
        // An invalid request gets no data string, whatever it is handed.
        write_decrpss(&mut buf, false, "0;1m").unwrap();
        assert_eq!(
            buf,
            b"\x1bP1$r0;4;5;7m\x1b\\\x1bP1$r1;24r\x1b\\\x1bP1$r>4;2m\x1b\\\x1bP0$r\x1b\\"
        );
    }

    #[test]
    fn test_dsr_request() {
        let mut buf = Vec::new();
        write_dsr_request(&mut buf, false, 5).unwrap();
        write_dsr_request(&mut buf, true, 996).unwrap();
        assert_eq!(buf, b"\x1b[5n\x1b[?996n");
    }

    #[test]
    fn test_visibility_report() {
        let mut buf = Vec::new();
        write_request_visibility_report(&mut buf).unwrap();
        assert_eq!(buf, b"\x1b[?998n");
        // The query is the generic DEC-private DSR 998.
        let mut generic = Vec::new();
        write_dsr_request(&mut generic, true, 998).unwrap();
        assert_eq!(buf, generic);

        let mut buf = Vec::new();
        write_visibility_report(&mut buf, true).unwrap();
        write_visibility_report(&mut buf, false).unwrap();
        assert_eq!(buf, b"\x1b[?999;1n\x1b[?999;2n");
    }
}

//! Default foreground, background, cursor, and indexed palette colors.
//!
//! ## Category
//!
//! This module emits OSC color-control sequences: OSC 10/11/12 for the default
//! text and cursor colors, OSC 4 for indexed palette entries, and OSC 104/110
//! through 112 for resets.
//!
//! ## OSC framing
//!
//! All writers and byte constants in this module use the 7-bit OSC introducer
//! and BEL terminator:
//!
//! ```text
//! ESC ]  10 ; rgb:ffff/0000/8080  BEL
//! ──┬──  ┬─   ─────────┬─────────  ─┬─
//!  OSC  code        payload       terminator
//! ```
//!
//! The `color` payload is passed through unchanged. Use [`xparse_rgb`] when a
//! caller needs the `rgb:RRRR/GGGG/BBBB` form accepted by these controls.
//!
//! ## Mode interaction
//!
//! These sequences do not require a DEC or ANSI mode to be enabled. Requests
//! cause compatible terminals to report colors asynchronously as OSC replies.

use std::io::{self, Write};

/// Set the default text foreground color with `ESC ] 10 ; <color> BEL`.
///
/// The `color` string is emitted verbatim. Use values accepted by the terminal, such as `#rrggbb` or [`xparse_rgb`] output. This changes the default color, not the current SGR foreground attribute.
pub fn write_set_foreground_color<W: Write>(w: &mut W, color: &str) -> io::Result<()> {
    write!(w, "\x1b]10;{color}\x07")
}

/// Set the default text background color with `ESC ] 11 ; <color> BEL`.
///
/// The `color` payload is emitted verbatim and typically uses an XParseColor-compatible color string. This changes the terminal default background rather than emitting SGR.
pub fn write_set_background_color<W: Write>(w: &mut W, color: &str) -> io::Result<()> {
    write!(w, "\x1b]11;{color}\x07")
}

/// Set the cursor color with `ESC ] 12 ; <color> BEL`.
///
/// The `color` payload is emitted verbatim. Use this when the cursor color should differ from the terminal theme default.
pub fn write_set_cursor_color<W: Write>(w: &mut W, color: &str) -> io::Result<()> {
    write!(w, "\x1b]12;{color}\x07")
}

/// Set one indexed palette entry with `ESC ] 4 ; <index> ; <color> BEL`.
///
/// `index` is written as a decimal palette index and `color` is emitted verbatim. Use [`xparse_rgb`] to build an `rgb:RRRR/GGGG/BBBB` payload from 8-bit channels.
pub fn write_set_palette_color<W: Write>(w: &mut W, index: u8, color: &str) -> io::Result<()> {
    write!(w, "\x1b]4;{index};{color}\x07")
}

/// Request the default foreground color: exact bytes `ESC ] 10 ; ? BEL` (`b"\x1b]10;?\x07"`).
///
/// A compatible terminal replies asynchronously with an OSC 10 color report.
pub const REQUEST_FOREGROUND_COLOR: &[u8] = b"\x1b]10;?\x07";

/// Request the default background color: exact bytes `ESC ] 11 ; ? BEL` (`b"\x1b]11;?\x07"`).
///
/// A compatible terminal replies asynchronously with an OSC 11 color report.
pub const REQUEST_BACKGROUND_COLOR: &[u8] = b"\x1b]11;?\x07";

/// Request the cursor color: exact bytes `ESC ] 12 ; ? BEL` (`b"\x1b]12;?\x07"`).
///
/// A compatible terminal replies asynchronously with an OSC 12 color report.
pub const REQUEST_CURSOR_COLOR: &[u8] = b"\x1b]12;?\x07";

/// Request one indexed palette entry with `ESC ] 4 ; <index> ; ? BEL`.
///
/// The terminal reply, when supported, uses OSC 4 with the same index and a color payload such as `rgb:RRRR/GGGG/BBBB`.
pub fn write_request_palette_color<W: Write>(w: &mut W, index: u8) -> io::Result<()> {
    write!(w, "\x1b]4;{index};?\x07")
}

/// Reset one indexed palette entry with `ESC ] 104 ; <index> BEL`.
///
/// `index` is written as a decimal palette index and the terminal restores that entry to its configured default.
pub fn write_reset_palette_color<W: Write>(w: &mut W, index: u8) -> io::Result<()> {
    write!(w, "\x1b]104;{index}\x07")
}

/// Reset all indexed palette colors: exact bytes `ESC ] 104 BEL` (`b"\x1b]104\x07"`).
pub const RESET_PALETTE_COLORS: &[u8] = b"\x1b]104\x07";

/// Reset the default foreground color: exact bytes `ESC ] 110 BEL` (`b"\x1b]110\x07"`).
pub const RESET_FOREGROUND_COLOR: &[u8] = b"\x1b]110\x07";

/// Reset the default background color: exact bytes `ESC ] 111 BEL` (`b"\x1b]111\x07"`).
pub const RESET_BACKGROUND_COLOR: &[u8] = b"\x1b]111\x07";

/// Reset the cursor color: exact bytes `ESC ] 112 BEL` (`b"\x1b]112\x07"`).
pub const RESET_CURSOR_COLOR: &[u8] = b"\x1b]112\x07";

/// Format 8-bit RGB channels as an XParseColor `rgb:RRRR/GGGG/BBBB` string.
///
/// Each channel is duplicated into 16-bit form (`0x80` becomes `8080`). The returned string is suitable for the color payloads accepted by this module.
pub fn xparse_rgb(r: u8, g: u8, b: u8) -> String {
    // Match the 16-bit channel convention (low byte == high byte).
    let r = (r as u16) << 8 | r as u16;
    let g = (g as u16) << 8 | g as u16;
    let b = (b as u16) << 8 | b as u16;
    format!("rgb:{r:04x}/{g:04x}/{b:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_fg() {
        let mut buf = Vec::new();
        write_set_foreground_color(&mut buf, "#ffffff").unwrap();
        assert_eq!(buf, b"\x1b]10;#ffffff\x07");
    }

    #[test]
    fn test_xparse_rgb() {
        assert_eq!(xparse_rgb(0xff, 0x00, 0x80), "rgb:ffff/0000/8080");
    }

    #[test]
    fn test_set_palette_color() {
        let mut buf = Vec::new();
        write_set_palette_color(&mut buf, 1, "rgb:ffff/0000/8080").unwrap();
        assert_eq!(buf, b"\x1b]4;1;rgb:ffff/0000/8080\x07");
    }

    #[test]
    fn test_request_palette_color() {
        let mut buf = Vec::new();
        write_request_palette_color(&mut buf, 5).unwrap();
        assert_eq!(buf, b"\x1b]4;5;?\x07");
    }

    #[test]
    fn test_reset_palette_color_one() {
        let mut buf = Vec::new();
        write_reset_palette_color(&mut buf, 5).unwrap();
        assert_eq!(buf, b"\x1b]104;5\x07");
    }

    #[test]
    fn test_reset_palette_color_all() {
        assert_eq!(RESET_PALETTE_COLORS, b"\x1b]104\x07");
    }
}

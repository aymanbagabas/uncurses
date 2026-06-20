//! Default terminal foreground / background / cursor colors (OSC 10/11/12).

use std::io::{self, Write};

/// Set the default terminal foreground color (`OSC 10 ; color ST`).
pub fn write_set_foreground_color<W: Write>(w: &mut W, color: &str) -> io::Result<()> {
    write!(w, "\x1b]10;{color}\x07")
}

/// Set the default terminal background color (`OSC 11 ; color ST`).
pub fn write_set_background_color<W: Write>(w: &mut W, color: &str) -> io::Result<()> {
    write!(w, "\x1b]11;{color}\x07")
}

/// Set the terminal cursor color (`OSC 12 ; color ST`).
pub fn write_set_cursor_color<W: Write>(w: &mut W, color: &str) -> io::Result<()> {
    write!(w, "\x1b]12;{color}\x07")
}

/// Set a terminal palette color by index (`OSC 4 ; index ; color ST`).
pub fn write_set_palette_color<W: Write>(w: &mut W, index: u8, color: &str) -> io::Result<()> {
    write!(w, "\x1b]4;{index};{color}\x07")
}

/// Request the current default foreground color (`OSC 10 ; ? ST`).
pub const REQUEST_FOREGROUND_COLOR: &[u8] = b"\x1b]10;?\x07";

/// Request the current default background color (`OSC 11 ; ? ST`).
pub const REQUEST_BACKGROUND_COLOR: &[u8] = b"\x1b]11;?\x07";

/// Request the current cursor color (`OSC 12 ; ? ST`).
pub const REQUEST_CURSOR_COLOR: &[u8] = b"\x1b]12;?\x07";

/// Request a terminal palette color by index (`OSC 4 ; index ; ? ST`).
/// The terminal replies with `OSC 4 ; index ; rgb:RRRR/GGGG/BBBB ST`.
pub fn write_request_palette_color<W: Write>(w: &mut W, index: u8) -> io::Result<()> {
    write!(w, "\x1b]4;{index};?\x07")
}

/// Reset a single palette color to its default (`OSC 104 ; index ST`).
pub fn write_reset_palette_color<W: Write>(w: &mut W, index: u8) -> io::Result<()> {
    write!(w, "\x1b]104;{index}\x07")
}

/// Reset the entire indexed color palette to defaults (`OSC 104 ST`).
pub const RESET_PALETTE_COLORS: &[u8] = b"\x1b]104\x07";

/// Reset default foreground color (`OSC 110 ST`).
pub const RESET_FOREGROUND_COLOR: &[u8] = b"\x1b]110\x07";

/// Reset default background color (`OSC 111 ST`).
pub const RESET_BACKGROUND_COLOR: &[u8] = b"\x1b]111\x07";

/// Reset cursor color (`OSC 112 ST`).
pub const RESET_CURSOR_COLOR: &[u8] = b"\x1b]112\x07";

/// Format an RGB color as an XParseColor `rgb:` string (`rgb:RRRR/GGGG/BBBB`),
/// suitable for passing to [`write_set_foreground_color`] et al.
pub fn xparse_rgb(r: u8, g: u8, b: u8) -> String {
    // Match xterm convention: 16-bit channel values (low byte == high byte).
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

//! Terminal color types and capability profiles.
//!
//! This module defines [`Color`], the standard [`BasicColor`] palette, and [`Profile`] for choosing how colors should be encoded.
//! Reach for it when styling text, converting palette entries to RGB, or adapting output to a terminal's color support.
//!
//! A [`Color`] is RGB, an indexed palette entry, or a named [`BasicColor`].
//! A [`Profile`] downsamples a color to what a terminal can render, so the
//! same UI code works on a true-color terminal and a 16-color one:
//!
//! ```
//! use uncurses::color::{BasicColor, Color, Profile};
//!
//! let orange = Color::Rgb(255, 128, 0);
//! let green: Color = BasicColor::Green.into();
//!
//! // TrueColor keeps the exact value; lesser profiles quantize it; the
//! // colorless profiles drop it entirely (`None`).
//! assert_eq!(Profile::TrueColor.convert(orange), Some(orange));
//! assert!(Profile::Ansi256.convert(orange).is_some());
//! assert_eq!(Profile::Disabled.convert(green), None);
//! ```

mod convert;
mod profile;

pub use profile::*;

/// A terminal color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// Standard 16 ANSI colors (0-15).
    Basic(BasicColor),
    /// Extended 256-color palette (0-255).
    Indexed(u8),
    /// 24-bit true color.
    Rgb(u8, u8, u8),
}

impl Color {
    /// Construct a 24-bit RGB color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::Rgb(r, g, b)
    }

    /// Convert this color to (r, g, b) components.
    pub fn to_rgb(self) -> (u8, u8, u8) {
        match self {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(idx) => indexed_to_rgb(idx),
            Color::Basic(c) => indexed_to_rgb(c.as_u8()),
        }
    }
}

/// The 16 standard ANSI colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BasicColor {
    /// Black.
    Black = 0,
    /// Red.
    Red = 1,
    /// Green.
    Green = 2,
    /// Yellow.
    Yellow = 3,
    /// Blue.
    Blue = 4,
    /// Magenta.
    Magenta = 5,
    /// Cyan.
    Cyan = 6,
    /// White.
    White = 7,
    /// Bright black.
    BrightBlack = 8,
    /// Bright red.
    BrightRed = 9,
    /// Bright green.
    BrightGreen = 10,
    /// Bright yellow.
    BrightYellow = 11,
    /// Bright blue.
    BrightBlue = 12,
    /// Bright magenta.
    BrightMagenta = 13,
    /// Bright cyan.
    BrightCyan = 14,
    /// Bright white.
    BrightWhite = 15,
}

impl BasicColor {
    /// Return the 0..=15 palette index.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Whether this is a bright/high-intensity color.
    pub const fn is_bright(self) -> bool {
        self as u8 >= 8
    }

    /// Convert a 0..=15 value to a `BasicColor`, returning `None` if out of range.
    pub const fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Black,
            1 => Self::Red,
            2 => Self::Green,
            3 => Self::Yellow,
            4 => Self::Blue,
            5 => Self::Magenta,
            6 => Self::Cyan,
            7 => Self::White,
            8 => Self::BrightBlack,
            9 => Self::BrightRed,
            10 => Self::BrightGreen,
            11 => Self::BrightYellow,
            12 => Self::BrightBlue,
            13 => Self::BrightMagenta,
            14 => Self::BrightCyan,
            15 => Self::BrightWhite,
            _ => return None,
        })
    }
}

impl From<BasicColor> for Color {
    fn from(c: BasicColor) -> Self {
        Color::Basic(c)
    }
}

/// The standard xterm 256-color palette RGB values.
const XTERM_COLORS: [(u8, u8, u8); 256] = {
    let mut table = [(0u8, 0u8, 0u8); 256];

    // 0-15: Standard colors (approximate)
    table[0] = (0, 0, 0);
    table[1] = (128, 0, 0);
    table[2] = (0, 128, 0);
    table[3] = (128, 128, 0);
    table[4] = (0, 0, 128);
    table[5] = (128, 0, 128);
    table[6] = (0, 128, 128);
    table[7] = (192, 192, 192);
    table[8] = (128, 128, 128);
    table[9] = (255, 0, 0);
    table[10] = (0, 255, 0);
    table[11] = (255, 255, 0);
    table[12] = (0, 0, 255);
    table[13] = (255, 0, 255);
    table[14] = (0, 255, 255);
    table[15] = (255, 255, 255);

    // 16-231: 6x6x6 color cube
    let cube_values: [u8; 6] = [0, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    let mut i = 16usize;
    let mut r = 0usize;
    while r < 6 {
        let mut g = 0usize;
        while g < 6 {
            let mut b = 0usize;
            while b < 6 {
                table[i] = (cube_values[r], cube_values[g], cube_values[b]);
                i += 1;
                b += 1;
            }
            g += 1;
        }
        r += 1;
    }

    // 232-255: Grayscale ramp
    let mut i = 232usize;
    while i < 256 {
        let v = (8 + 10 * (i - 232)) as u8;
        table[i] = (v, v, v);
        i += 1;
    }

    table
};

/// Look up the RGB values for a 256-color palette index.
pub fn indexed_to_rgb(idx: u8) -> (u8, u8, u8) {
    XTERM_COLORS[idx as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_color_as_u8() {
        assert_eq!(BasicColor::Black.as_u8(), 0);
        assert_eq!(BasicColor::BrightWhite.as_u8(), 15);
    }

    #[test]
    fn test_color_to_rgb() {
        assert_eq!(Color::Rgb(255, 128, 0).to_rgb(), (255, 128, 0));
        assert_eq!(Color::Basic(BasicColor::Black).to_rgb(), (0, 0, 0));
    }

    #[test]
    fn test_xterm_cube_color() {
        // Index 196 = pure red in the 6x6x6 cube (5,0,0)
        assert_eq!(indexed_to_rgb(196), (255, 0, 0));
    }

    #[test]
    fn test_xterm_grayscale() {
        assert_eq!(indexed_to_rgb(232), (8, 8, 8));
        assert_eq!(indexed_to_rgb(255), (238, 238, 238));
    }
}

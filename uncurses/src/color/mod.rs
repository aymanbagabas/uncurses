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

    /// Format this color as a `#rrggbb` hex string.
    ///
    /// Palette and indexed colors are resolved to their RGB values first, so
    /// the result is always the six-digit true-color form.
    pub fn to_hex(self) -> String {
        let (r, g, b) = self.to_rgb();
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// Convert this color to HSL components.
    ///
    /// Returns `(h, s, l)` with the hue `h` in degrees (`0.0..360.0`) and the
    /// saturation `s` and lightness `l` in `0.0..=1.0`. Palette and indexed
    /// colors are resolved to RGB first.
    pub fn to_hsl(self) -> (f32, f32, f32) {
        let (r, g, b) = self.to_rgb();
        let r = f32::from(r) / 255.0;
        let g = f32::from(g) / 255.0;
        let b = f32::from(b) / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let l = (max + min) / 2.0;
        if delta == 0.0 {
            return (0.0, 0.0, l);
        }
        let s = delta / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            60.0 * ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        (h, s, l)
    }

    /// Parse a hex color string into an [`Color::Rgb`].
    ///
    /// The leading `#` is optional. Accepts three forms:
    ///
    /// - 3 digits (`#fff`): shorthand, each nibble is doubled (`f` -> `ff`).
    /// - 6 digits (`#ffffff`): one byte per channel.
    /// - 8 digits (`#ffffff00`): a trailing alpha pair is parsed but ignored,
    ///   since colors carry no alpha channel.
    ///
    /// Returns `None` for any other length or for non-hexadecimal input.
    pub fn hex(s: &str) -> Option<Color> {
        let s = s.strip_prefix('#').unwrap_or(s);
        let bytes = s.as_bytes();
        let (r, g, b) = match bytes.len() {
            3 => {
                let r = hex_nibble(bytes[0])?;
                let g = hex_nibble(bytes[1])?;
                let b = hex_nibble(bytes[2])?;
                (r * 17, g * 17, b * 17)
            }
            6 | 8 => {
                let r = hex_byte(bytes[0], bytes[1])?;
                let g = hex_byte(bytes[2], bytes[3])?;
                let b = hex_byte(bytes[4], bytes[5])?;
                (r, g, b)
            }
            _ => return None,
        };
        Some(Color::Rgb(r, g, b))
    }

    /// Construct an [`Color::Rgb`] from HSL.
    ///
    /// `h` is the hue in degrees (wrapped into `0..360`); `s` (saturation) and
    /// `l` (lightness) are clamped to `0.0..=1.0`.
    pub fn hsl(h: f32, s: f32, l: f32) -> Color {
        let h = h.rem_euclid(360.0);
        let s = s.clamp(0.0, 1.0);
        let l = l.clamp(0.0, 1.0);
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let hp = h / 60.0;
        let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
        let (r1, g1, b1) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        let to = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Color::Rgb(to(r1), to(g1), to(b1))
    }
}

/// Parse one ASCII hex digit into its `0..16` value.
fn hex_nibble(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Parse two ASCII hex digits into a byte.
fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    Some(hex_nibble(hi)? * 16 + hex_nibble(lo)?)
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

impl From<BasicColor> for Option<Color> {
    fn from(c: BasicColor) -> Self {
        Some(Color::Basic(c))
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

    #[test]
    fn test_hex_forms() {
        assert_eq!(Color::hex("#fff"), Some(Color::Rgb(255, 255, 255)));
        assert_eq!(Color::hex("fff"), Some(Color::Rgb(255, 255, 255)));
        assert_eq!(Color::hex("#abc"), Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(Color::hex("#ff8800"), Some(Color::Rgb(255, 136, 0)));
        // 8-digit form: the trailing alpha pair is parsed but ignored.
        assert_eq!(Color::hex("#ff880042"), Some(Color::Rgb(255, 136, 0)));
    }

    #[test]
    fn test_hex_rejects_bad_input() {
        assert_eq!(Color::hex("#ff"), None); // wrong length
        assert_eq!(Color::hex("#fffff"), None); // wrong length
        assert_eq!(Color::hex("#gggggg"), None); // non-hex digits
        assert_eq!(Color::hex(""), None);
    }

    #[test]
    fn test_hsl() {
        assert_eq!(Color::hsl(0.0, 1.0, 0.5), Color::Rgb(255, 0, 0));
        assert_eq!(Color::hsl(120.0, 1.0, 0.5), Color::Rgb(0, 255, 0));
        assert_eq!(Color::hsl(240.0, 1.0, 0.5), Color::Rgb(0, 0, 255));
        // Hue wraps and full lightness is white regardless of hue.
        assert_eq!(Color::hsl(360.0, 1.0, 0.5), Color::Rgb(255, 0, 0));
        assert_eq!(Color::hsl(123.0, 0.5, 1.0), Color::Rgb(255, 255, 255));
        assert_eq!(Color::hsl(200.0, 0.7, 0.0), Color::Rgb(0, 0, 0));
    }

    #[test]
    fn test_to_hex() {
        assert_eq!(Color::Rgb(255, 136, 0).to_hex(), "#ff8800");
        assert_eq!(Color::Rgb(0, 0, 0).to_hex(), "#000000");
        assert_eq!(Color::Basic(BasicColor::Black).to_hex(), "#000000");
        // Round-trips with `hex`.
        assert_eq!(
            Color::hex(&Color::Rgb(18, 52, 86).to_hex()),
            Some(Color::Rgb(18, 52, 86))
        );
    }

    #[test]
    fn test_to_hsl() {
        let approx = |a: f32, b: f32| (a - b).abs() < 0.01;
        let (h, s, l) = Color::Rgb(255, 0, 0).to_hsl();
        assert!(approx(h, 0.0) && approx(s, 1.0) && approx(l, 0.5));
        let (h, s, l) = Color::Rgb(0, 255, 0).to_hsl();
        assert!(approx(h, 120.0) && approx(s, 1.0) && approx(l, 0.5));
        let (h, s, l) = Color::Rgb(0, 0, 255).to_hsl();
        assert!(approx(h, 240.0) && approx(s, 1.0) && approx(l, 0.5));
        // Gray: undefined hue reported as 0, zero saturation.
        let (_, s, l) = Color::Rgb(128, 128, 128).to_hsl();
        assert!(approx(s, 0.0) && approx(l, 128.0 / 255.0));
        // hsl -> rgb -> hsl round-trip stays close.
        let (h, s, l) = Color::hsl(210.0, 0.6, 0.45).to_hsl();
        assert!(approx(h, 210.0) && approx(s, 0.6) && approx(l, 0.45));
    }

    #[test]
    fn test_basic_color_into_option() {
        let c: Option<Color> = BasicColor::Red.into();
        assert_eq!(c, Some(Color::Basic(BasicColor::Red)));
    }
}

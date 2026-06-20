//! Terminal color values, palettes, and capability profiles.
//!
//! ## Color values
//!
//! [`Color`] has three representations: [`Color::Basic`] for the standard
//! 16-color ANSI palette, [`Color::Indexed`] for the xterm 256-color palette,
//! and [`Color::Rgb`] for 24-bit true color. All three can be converted to RGB
//! with [`Color::to_rgb`], which makes palette colors usable with true-color
//! helpers such as [`Color::to_hex`] and [`Color::to_hsl`].
//!
//! ## Basic colors
//!
//! [`BasicColor`] is the named 16-color ANSI palette. Its discriminants are
//! the palette indices `0..=15`; `0..=7` are normal intensity and `8..=15` are
//! bright/high-intensity variants. Converting a [`BasicColor`] into [`Color`]
//! yields [`Color::Basic`], and converting into `Option<Color>` yields
//! `Some(Color::Basic(_))` for builder ergonomics.
//!
//! ## Hex and HSL helpers
//!
//! [`Color::hex`] accepts optional-`#` three-, six-, or eight-digit hex input
//! and returns [`Color::Rgb`]. Eight-digit input parses and ignores the final
//! alpha byte because terminal colors carry no alpha channel. [`Color::hsl`]
//! wraps hue into `0..360`, clamps saturation/lightness into `0.0..=1.0`, and
//! rounds computed RGB channels to the nearest byte.
//!
//! [`Color::to_hex`] and [`Color::to_hsl`] first resolve basic/indexed colors
//! through the xterm palette. Gray HSL colors report hue `0.0` because the hue
//! is undefined when saturation is zero.
//!
//! ## Profile downsampling
//!
//! [`Profile`] describes the color capability of an output stream. Converting a
//! color through a profile preserves true color when possible, quantizes to the
//! nearest supported palette for `Ansi256`/`Ansi`, and drops color entirely for
//! `Ascii`/`Disabled`.
//!
//! ```text
//! Color::Rgb / Indexed / Basic
//!          │
//!          ├─ Profile::TrueColor ──► original color
//!          ├─ Profile::Ansi256   ──► nearest xterm index
//!          ├─ Profile::Ansi      ──► nearest BasicColor
//!          └─ Ascii / Disabled   ──► None
//! ```
//!
//! ```rust,ignore
//! use uncurses::color::{BasicColor, Color, Profile};
//!
//! let orange = Color::Rgb(255, 128, 0);
//! let green: Color = BasicColor::Green.into();
//!
//! assert_eq!(Profile::TrueColor.convert(orange), Some(orange));
//! assert!(matches!(Profile::Ansi256.convert(orange), Some(Color::Indexed(_))));
//! assert_eq!(Profile::Disabled.convert(green), None);
//! ```

mod convert;
mod profile;

pub use profile::*;

/// A terminal color in one of the supported palette spaces.
///
/// Use [`Color::Rgb`] when exact 24-bit color should be preserved on
/// true-color terminals, [`Color::Indexed`] when targeting a specific xterm
/// 256-color palette entry, and [`Color::Basic`] for the portable 16-color ANSI
/// palette. A [`Profile`] can downsample any variant to the capability of a
/// particular output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// Standard 16-color ANSI palette entry.
    Basic(BasicColor),
    /// Extended xterm 256-color palette index (`0..=255`).
    Indexed(u8),
    /// 24-bit true color as red, green, and blue bytes.
    Rgb(u8, u8, u8),
}

impl Color {
    /// Construct a 24-bit RGB color.
    ///
    /// Returns [`Color::Rgb`] with the supplied red, green, and blue bytes.
    /// This is a `const` convenience constructor for code that prefers named
    /// parameters over the enum variant.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::Rgb(r, g, b)
    }

    /// Convert this color to `(red, green, blue)` bytes.
    ///
    /// [`Color::Rgb`] returns its stored components unchanged. [`Color::Basic`]
    /// and [`Color::Indexed`] are resolved through the xterm 256-color palette,
    /// where basic colors use their `0..=15` palette index.
    pub fn to_rgb(self) -> (u8, u8, u8) {
        match self {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(idx) => indexed_to_rgb(idx),
            Color::Basic(c) => indexed_to_rgb(c.as_u8()),
        }
    }

    /// Format this color as a lower-case `#rrggbb` hex string.
    ///
    /// Palette and indexed colors are resolved with [`Color::to_rgb`] first,
    /// so the result is always the six-digit true-color form. The returned
    /// string never includes alpha.
    pub fn to_hex(self) -> String {
        let (r, g, b) = self.to_rgb();
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// Convert this color to HSL components.
    ///
    /// Returns `(h, s, l)` with hue in degrees (`0.0..360.0`) and saturation
    /// and lightness in `0.0..=1.0`. Palette and indexed colors are resolved to
    /// RGB first. If the color is gray (`r == g == b`), saturation is `0.0` and
    /// hue is reported as `0.0`.
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

    /// Parse a hex color string into [`Color::Rgb`].
    ///
    /// The leading `#` is optional. Accepted forms are:
    ///
    /// - `rgb` / `#rgb`: shorthand, each nibble is doubled (`f` becomes `ff`).
    /// - `rrggbb` / `#rrggbb`: one byte per channel.
    /// - `rrggbbaa` / `#rrggbbaa`: a trailing alpha byte is parsed for
    ///   validity but ignored.
    ///
    /// Returns `None` for any other length or for non-hexadecimal input. The
    /// function does not panic.
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
                // For the 8-digit form, validate the trailing alpha pair so
                // non-hex input is rejected, then discard it: terminal colors
                // carry no alpha channel.
                if bytes.len() == 8 {
                    hex_byte(bytes[6], bytes[7])?;
                }
                (r, g, b)
            }
            _ => return None,
        };
        Some(Color::Rgb(r, g, b))
    }

    /// Construct [`Color::Rgb`] from HSL.
    ///
    /// `h` is hue in degrees and is wrapped into `0.0..360.0` with
    /// `f32::rem_euclid`. `s` (saturation) and `l` (lightness) are clamped to
    /// `0.0..=1.0`. The intermediate RGB values are rounded to the nearest byte
    /// and clamped to `0..=255`.
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

/// Parse one ASCII hex digit into its `0..=15` value.
fn hex_nibble(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Parse two ASCII hex digits into a byte.
fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    Some(hex_nibble(hi)? * 16 + hex_nibble(lo)?)
}

/// The 16 standard ANSI colors.
///
/// The numeric value of each variant is its ANSI/xterm palette index. Normal
/// colors occupy `0..=7`; bright colors occupy `8..=15` and encode as bright
/// foreground/background SGR parameters when used in a [`Style`](crate::style::Style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BasicColor {
    /// Black (`0`, foreground `30`, background `40`).
    Black = 0,
    /// Red (`1`, foreground `31`, background `41`).
    Red = 1,
    /// Green (`2`, foreground `32`, background `42`).
    Green = 2,
    /// Yellow (`3`, foreground `33`, background `43`).
    Yellow = 3,
    /// Blue (`4`, foreground `34`, background `44`).
    Blue = 4,
    /// Magenta (`5`, foreground `35`, background `45`).
    Magenta = 5,
    /// Cyan (`6`, foreground `36`, background `46`).
    Cyan = 6,
    /// White (`7`, foreground `37`, background `47`).
    White = 7,
    /// Bright black (`8`, foreground `90`, background `100`).
    BrightBlack = 8,
    /// Bright red (`9`, foreground `91`, background `101`).
    BrightRed = 9,
    /// Bright green (`10`, foreground `92`, background `102`).
    BrightGreen = 10,
    /// Bright yellow (`11`, foreground `93`, background `103`).
    BrightYellow = 11,
    /// Bright blue (`12`, foreground `94`, background `104`).
    BrightBlue = 12,
    /// Bright magenta (`13`, foreground `95`, background `105`).
    BrightMagenta = 13,
    /// Bright cyan (`14`, foreground `96`, background `106`).
    BrightCyan = 14,
    /// Bright white (`15`, foreground `97`, background `107`).
    BrightWhite = 15,
}

impl BasicColor {
    /// Return the `0..=15` ANSI palette index.
    ///
    /// This value is also the xterm 256-color palette index used when
    /// resolving a basic color through [`Color::to_rgb`].
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Return whether this is a bright/high-intensity color.
    ///
    /// Bright colors are variants with indices `8..=15` and encode as
    /// foreground SGR `90..=97` or background SGR `100..=107`.
    pub const fn is_bright(self) -> bool {
        self as u8 >= 8
    }

    /// Convert a palette index to a [`BasicColor`].
    ///
    /// Returns `Some` for values in `0..=15` and `None` for any higher value.
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
    /// Convert a [`BasicColor`] into [`Color::Basic`].
    fn from(c: BasicColor) -> Self {
        Color::Basic(c)
    }
}

impl From<BasicColor> for Option<Color> {
    /// Convert a [`BasicColor`] into `Some(Color::Basic(_))`.
    ///
    /// This supports style builders that accept `impl Into<Option<Color>>`.
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

/// Look up the RGB values for an xterm 256-color palette index.
///
/// Indices `0..=15` are the standard ANSI colors, `16..=231` are the 6×6×6
/// color cube, and `232..=255` are the grayscale ramp. Every `u8` is a valid
/// palette index, so this function does not fail or panic.
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
        assert_eq!(Color::hex("#ff8800zz"), None); // non-hex alpha pair
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

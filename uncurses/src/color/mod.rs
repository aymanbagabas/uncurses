//! Terminal color values, palettes, and capability profiles.
//!
//! ## Color values
//!
//! [`Color`] covers three palette spaces in one enum: the sixteen named ANSI
//! colors ([`Color::Green`], [`Color::BrightBlue`], and so on),
//! [`Color::Indexed`] for the xterm 256-color palette, and [`Color::Rgb`] for
//! 24-bit true color. All of them convert to RGB with [`Color::to_rgb`], which
//! makes palette colors usable with true-color helpers such as
//! [`Color::to_hex`] and [`Color::to_hsl`].
//!
//! ## Named colors
//!
//! The sixteen named variants are the standard ANSI palette: `Black` through
//! `White` are normal intensity (palette indices `0..=7`) and `BrightBlack`
//! through `BrightWhite` are the bright/high-intensity variants (`8..=15`).
//! [`Color::named_index`] returns that `0..=15` index for a named color and
//! `None` for [`Color::Indexed`]/[`Color::Rgb`]; [`Color::from_named`]
//! goes the other way.
//!
//! ## Hex and HSL helpers
//!
//! [`Color::hex`] accepts optional-`#` three-, six-, or eight-digit hex input
//! and returns [`Color::Rgb`]. Eight-digit input parses and ignores the final
//! alpha byte because terminal colors carry no alpha channel. [`Color::hsl`]
//! wraps hue into `0..360`, clamps saturation/lightness into `0.0..=1.0`, and
//! rounds computed RGB channels to the nearest byte.
//!
//! [`Color::to_hex`] and [`Color::to_hsl`] first resolve named and indexed
//! colors through the xterm palette. Gray HSL colors report hue `0.0` because
//! the hue is undefined when saturation is zero.
//!
//! ## Profile downsampling
//!
//! [`Profile`] describes the color capability of an output stream. Converting a
//! color through a profile preserves true color when possible, quantizes to the
//! nearest supported palette for `Ansi256`/`Ansi`, and drops color entirely for
//! `Ascii`/`Disabled`.
//!
//! ```text
//! Color::Rgb / Indexed / named
//!          │
//!          ├─ Profile::TrueColor ──► original color
//!          ├─ Profile::Ansi256   ──► nearest xterm index
//!          ├─ Profile::Ansi      ──► nearest named color
//!          └─ Ascii / Disabled   ──► None
//! ```
//!
//! ```rust,ignore
//! use uncurses::color::{Color, Profile};
//!
//! let orange = Color::Rgb(255, 128, 0);
//! let green = Color::Green;
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
/// The sixteen named variants are the standard ANSI palette. Use
/// [`Color::Rgb`] when exact 24-bit color should be preserved on true-color
/// terminals and [`Color::Indexed`] when targeting a specific xterm 256-color
/// palette entry. A [`Profile`] can downsample any variant to the capability
/// of a particular output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// Black (palette `0`, foreground `30`, background `40`).
    Black,
    /// Red (palette `1`, foreground `31`, background `41`).
    Red,
    /// Green (palette `2`, foreground `32`, background `42`).
    Green,
    /// Yellow (palette `3`, foreground `33`, background `43`).
    Yellow,
    /// Blue (palette `4`, foreground `34`, background `44`).
    Blue,
    /// Magenta (palette `5`, foreground `35`, background `45`).
    Magenta,
    /// Cyan (palette `6`, foreground `36`, background `46`).
    Cyan,
    /// White (palette `7`, foreground `37`, background `47`).
    White,
    /// Bright black (palette `8`, foreground `90`, background `100`).
    BrightBlack,
    /// Bright red (palette `9`, foreground `91`, background `101`).
    BrightRed,
    /// Bright green (palette `10`, foreground `92`, background `102`).
    BrightGreen,
    /// Bright yellow (palette `11`, foreground `93`, background `103`).
    BrightYellow,
    /// Bright blue (palette `12`, foreground `94`, background `104`).
    BrightBlue,
    /// Bright magenta (palette `13`, foreground `95`, background `105`).
    BrightMagenta,
    /// Bright cyan (palette `14`, foreground `96`, background `106`).
    BrightCyan,
    /// Bright white (palette `15`, foreground `97`, background `107`).
    BrightWhite,
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

    /// Return the `0..=15` ANSI palette index for a named color.
    ///
    /// Returns `Some(0..=15)` for the sixteen named variants and `None` for
    /// [`Color::Indexed`] and [`Color::Rgb`]. The index is also the xterm
    /// 256-color palette index used when resolving a named color through
    /// [`Color::to_rgb`].
    pub const fn named_index(self) -> Option<u8> {
        Some(match self {
            Color::Black => 0,
            Color::Red => 1,
            Color::Green => 2,
            Color::Yellow => 3,
            Color::Blue => 4,
            Color::Magenta => 5,
            Color::Cyan => 6,
            Color::White => 7,
            Color::BrightBlack => 8,
            Color::BrightRed => 9,
            Color::BrightGreen => 10,
            Color::BrightYellow => 11,
            Color::BrightBlue => 12,
            Color::BrightMagenta => 13,
            Color::BrightCyan => 14,
            Color::BrightWhite => 15,
            Color::Indexed(_) | Color::Rgb(..) => return None,
        })
    }

    /// Convert a `0..=15` ANSI palette index into the matching named color.
    ///
    /// Returns `Some` for values in `0..=15` and `None` for any higher value.
    pub const fn from_named(v: u8) -> Option<Self> {
        Some(match v {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::White,
            8 => Color::BrightBlack,
            9 => Color::BrightRed,
            10 => Color::BrightGreen,
            11 => Color::BrightYellow,
            12 => Color::BrightBlue,
            13 => Color::BrightMagenta,
            14 => Color::BrightCyan,
            15 => Color::BrightWhite,
            _ => return None,
        })
    }

    /// Return whether this is one of the sixteen named ANSI colors.
    pub const fn is_named(self) -> bool {
        self.named_index().is_some()
    }

    /// Return whether this is a bright/high-intensity named color.
    ///
    /// Bright colors are the named variants with palette indices `8..=15` and
    /// encode as foreground SGR `90..=97` or background SGR `100..=107`.
    /// [`Color::Indexed`] and [`Color::Rgb`] are never bright.
    pub const fn is_named_bright(self) -> bool {
        matches!(self.named_index(), Some(8..=15))
    }

    /// Convert this color to `(red, green, blue)` bytes.
    ///
    /// [`Color::Rgb`] returns its stored components unchanged. Named colors and
    /// [`Color::Indexed`] are resolved through the xterm 256-color palette,
    /// where named colors use their `0..=15` palette index.
    pub fn to_rgb(self) -> (u8, u8, u8) {
        match self {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(idx) => indexed_to_rgb(idx),
            other => indexed_to_rgb(other.named_index().unwrap_or(0)),
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
    fn named_index_round_trips() {
        assert_eq!(Color::Black.named_index(), Some(0));
        assert_eq!(Color::BrightWhite.named_index(), Some(15));
        assert_eq!(Color::Indexed(42).named_index(), None);
        assert_eq!(Color::Rgb(1, 2, 3).named_index(), None);
        assert_eq!(Color::from_named(0), Some(Color::Black));
        assert_eq!(Color::from_named(15), Some(Color::BrightWhite));
        assert_eq!(Color::from_named(16), None);
        assert!(Color::Green.is_named());
        assert!(!Color::Indexed(2).is_named());
        assert!(Color::BrightRed.is_named_bright());
        assert!(!Color::Red.is_named_bright());
    }

    #[test]
    fn test_color_to_rgb() {
        assert_eq!(Color::Rgb(255, 128, 0).to_rgb(), (255, 128, 0));
        assert_eq!(Color::Black.to_rgb(), (0, 0, 0));
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
        assert_eq!(Color::Black.to_hex(), "#000000");
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
    fn color_into_option() {
        let c: Option<Color> = Color::Red.into();
        assert_eq!(c, Some(Color::Red));
    }
}

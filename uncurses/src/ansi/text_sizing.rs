//! Kitty OSC 66 text sizing.
//!
//! [`write_sized_text`] encodes text at the current cursor position.
//! Cursor placement and frame state belong to the screen's renderer.
//!
//! See <https://sw.kovidgoyal.net/kitty/text-sizing-protocol/>.

use std::io::{self, Write};

use crate::layout::Size;

/// Geometry and fractional font size for one Kitty OSC 66 block.
///
/// An explicit block occupies `scale * width` columns and `scale` rows.
/// Fractional scale changes the font inside that footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextSizing {
    /// Overall scale, from 1 through 7.
    pub scale: u8,
    /// Width before scaling, from 0 through 7.
    ///
    /// Zero asks the terminal to segment and measure the text.
    /// Screen regions require an explicit width from 1 through 7.
    pub width: u8,
    /// Fractional scale numerator, from 0 through 15.
    pub numerator: u8,
    /// Fractional scale denominator, from 0 through 15.
    /// When nonzero, it must exceed `numerator`.
    pub denominator: u8,
    /// Vertical alignment: 0 for top, 1 for bottom, 2 for center.
    pub vertical: u8,
    /// Horizontal alignment: 0 for left, 1 for right, 2 for center.
    pub horizontal: u8,
}

impl Default for TextSizing {
    fn default() -> Self {
        Self {
            scale: 1,
            width: 1,
            numerator: 0,
            denominator: 0,
            vertical: 0,
            horizontal: 0,
        }
    }
}

impl TextSizing {
    fn validate(self) -> io::Result<()> {
        if !(1..=7).contains(&self.scale)
            || self.width > 7
            || self.numerator > 15
            || self.denominator > 15
            || (self.denominator != 0 && self.denominator <= self.numerator)
            || self.vertical > 2
            || self.horizontal > 2
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid text sizing metadata",
            ));
        }
        Ok(())
    }

    /// Validate an explicit width and return its exact footprint.
    ///
    /// Automatic width (`width = 0`) returns `InvalidInput`.
    pub fn size(self) -> io::Result<Size> {
        self.validate()?;
        if self.width == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sized text regions require an explicit width",
            ));
        }
        Ok(Size::new(
            u16::from(self.scale) * u16::from(self.width),
            u16::from(self.scale),
        ))
    }
}

pub(crate) fn validate_text(text: &str) -> io::Result<()> {
    if text.len() > 4096 || text.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sized text must be escape-code safe and at most 4096 UTF-8 bytes",
        ));
    }
    Ok(())
}

/// Write one OSC 66 sequence at the current cursor position.
///
/// Text must be escape-code-safe UTF-8 with at most 4096 bytes.
/// Empty text encodes an explicit space so the terminal advances the cursor.
/// A leading combining character can join existing terminal content; screen
/// regions instead require an independent block.
///
/// Invalid metadata or text returns `InvalidInput` before any bytes are written.
/// Writer errors are propagated. This function does not flush.
pub fn write_sized_text<W: Write + ?Sized>(
    out: &mut W,
    text: &str,
    sizing: TextSizing,
) -> io::Result<()> {
    sizing.validate()?;
    validate_text(text)?;
    write!(
        out,
        "\x1b]66;s={}:w={}:n={}:d={}:v={}:h={};{}\x1b\\",
        sizing.scale,
        sizing.width,
        sizing.numerator,
        sizing.denominator,
        sizing.vertical,
        sizing.horizontal,
        if text.is_empty() { " " } else { text },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_explicit_and_automatic_width_without_cursor_moves() {
        for width in [0, 1, 7] {
            let mut bytes = Vec::new();
            write_sized_text(
                &mut bytes,
                "ab",
                TextSizing {
                    width,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                bytes,
                format!("\x1b]66;s=1:w={width}:n=0:d=0:v=0:h=0;ab\x1b\\").as_bytes()
            );
        }
    }

    #[test]
    fn invalid_input_preserves_output() {
        for (text, sizing) in [
            ("\x07", TextSizing::default()),
            (
                "valid",
                TextSizing {
                    scale: 8,
                    ..Default::default()
                },
            ),
            (
                "valid",
                TextSizing {
                    numerator: 2,
                    denominator: 1,
                    ..Default::default()
                },
            ),
        ] {
            let mut bytes = b"prefix".to_vec();
            assert!(write_sized_text(&mut bytes, text, sizing).is_err());
            assert_eq!(bytes, b"prefix");
        }
    }

    #[test]
    fn empty_text_encodes_a_space() {
        let mut bytes = Vec::new();
        write_sized_text(&mut bytes, "", TextSizing::default()).unwrap();
        assert_eq!(bytes, b"\x1b]66;s=1:w=1:n=0:d=0:v=0:h=0; \x1b\\");
    }
}

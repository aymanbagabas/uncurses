pub mod diff;
pub mod parse;
pub mod sgr;

pub use diff::*;
pub use parse::*;
pub use sgr::*;

use bitflags::bitflags;

use crate::color::Color;

bitflags! {
    /// Text attribute flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct AttrFlags: u16 {
        const BOLD          = 0b0000_0000_0001;
        const FAINT         = 0b0000_0000_0010;
        const ITALIC        = 0b0000_0000_0100;
        const SLOW_BLINK    = 0b0000_0000_1000;
        const RAPID_BLINK   = 0b0000_0001_0000;
        const REVERSE       = 0b0000_0010_0000;
        const CONCEAL       = 0b0000_0100_0000;
        const STRIKETHROUGH = 0b0000_1000_0000;
    }
}

/// Underline style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum UnderlineStyle {
    #[default]
    None = 0,
    Single = 1,
    Double = 2,
    Curly = 3,
    Dotted = 4,
    Dashed = 5,
}

/// A complete text style: colors, attributes, and underline style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub underline_color: Option<Color>,
    pub underline: UnderlineStyle,
    pub attrs: AttrFlags,
}

impl Style {
    pub const EMPTY: Style = Style {
        fg: None,
        bg: None,
        underline_color: None,
        underline: UnderlineStyle::None,
        attrs: AttrFlags::empty(),
    };

    /// Whether this style has any non-default attributes.
    pub fn is_empty(&self) -> bool {
        *self == Self::EMPTY
    }

    pub fn bold(mut self) -> Self {
        self.attrs |= AttrFlags::BOLD;
        self
    }

    pub fn faint(mut self) -> Self {
        self.attrs |= AttrFlags::FAINT;
        self
    }

    pub fn italic(mut self) -> Self {
        self.attrs |= AttrFlags::ITALIC;
        self
    }

    pub fn underline(mut self) -> Self {
        self.underline = UnderlineStyle::Single;
        self
    }

    pub fn strikethrough(mut self) -> Self {
        self.attrs |= AttrFlags::STRIKETHROUGH;
        self
    }

    pub fn reverse(mut self) -> Self {
        self.attrs |= AttrFlags::REVERSE;
        self
    }

    pub fn conceal(mut self) -> Self {
        self.attrs |= AttrFlags::CONCEAL;
        self
    }

    pub fn with_fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    pub fn with_bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    pub fn with_underline_color(mut self, color: Color) -> Self {
        self.underline_color = Some(color);
        self
    }

    pub fn with_underline_style(mut self, style: UnderlineStyle) -> Self {
        self.underline = style;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::BasicColor;

    #[test]
    fn test_style_empty() {
        assert!(Style::EMPTY.is_empty());
        assert!(Style::default().is_empty());
    }

    #[test]
    fn test_style_builder() {
        let s = Style::EMPTY
            .bold()
            .italic()
            .with_fg(Color::Basic(BasicColor::Red));
        assert!(s.attrs.contains(AttrFlags::BOLD));
        assert!(s.attrs.contains(AttrFlags::ITALIC));
        assert_eq!(s.fg, Some(Color::Basic(BasicColor::Red)));
        assert!(!s.is_empty());
    }
}

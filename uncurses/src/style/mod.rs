pub mod diff;
pub mod parse;
pub mod sgr;

pub use diff::*;
pub use parse::*;
pub use sgr::*;

use std::rc::Rc;

use bitflags::bitflags;

use crate::color::Color;

/// Hyperlink target carried by a [`Style`]. Stored behind an [`Rc`]
/// so cells in a hyperlink span share a single allocation. Private
/// to the style module — public callers interact with hyperlinks
/// through [`Style::with_link`] and [`Style::link`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LinkData {
    url: String,
    params: String,
}

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

/// A complete text style: colors, attributes, underline style, and
/// optional hyperlink target. Cloning a styled cell only bumps the
/// shared link refcount, so a long span of identically-linked cells
/// keeps a single allocation.
#[derive(Debug, Clone, Default)]
pub struct Style {
    pub(crate) fg: Option<Color>,
    pub(crate) bg: Option<Color>,
    pub(crate) underline_color: Option<Color>,
    pub(crate) underline: UnderlineStyle,
    pub(crate) attrs: AttrFlags,
    pub(crate) link: Option<Rc<LinkData>>,
}

impl PartialEq for Style {
    fn eq(&self, other: &Self) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.underline_color == other.underline_color
            && self.underline == other.underline
            && self.attrs == other.attrs
            && match (&self.link, &other.link) {
                (None, None) => true,
                (Some(a), Some(b)) => Rc::ptr_eq(a, b) || **a == **b,
                _ => false,
            }
    }
}

impl Eq for Style {}

impl std::hash::Hash for Style {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.fg.hash(state);
        self.bg.hash(state);
        self.underline_color.hash(state);
        self.underline.hash(state);
        self.attrs.hash(state);
        if let Some(l) = &self.link {
            l.hash(state);
        }
    }
}

impl Style {
    pub const EMPTY: Style = Style {
        fg: None,
        bg: None,
        underline_color: None,
        underline: UnderlineStyle::None,
        attrs: AttrFlags::empty(),
        link: None,
    };

    /// Whether this style is entirely empty — no colors, attributes,
    /// underline, or hyperlink. Equivalent to `*self == Style::EMPTY`.
    pub fn is_empty(&self) -> bool {
        self.is_sgr_empty() && self.is_link_empty()
    }

    /// Whether this style has no SGR-relevant settings (colors,
    /// attributes, underline). The hyperlink is intentionally
    /// ignored — it's orthogonal to SGR and emitted via OSC 8.
    pub(crate) fn is_sgr_empty(&self) -> bool {
        self.fg.is_none()
            && self.bg.is_none()
            && self.underline_color.is_none()
            && self.underline == UnderlineStyle::None
            && self.attrs.is_empty()
    }

    /// Whether this style carries no hyperlink. Companion to
    /// [`Style::is_sgr_empty`]; together they decide whether the
    /// style would emit any bytes at all.
    pub(crate) fn is_link_empty(&self) -> bool {
        self.link.is_none()
    }

    pub fn with_bold(mut self) -> Self {
        self.attrs |= AttrFlags::BOLD;
        self
    }

    pub fn with_faint(mut self) -> Self {
        self.attrs |= AttrFlags::FAINT;
        self
    }

    pub fn with_italic(mut self) -> Self {
        self.attrs |= AttrFlags::ITALIC;
        self
    }

    pub fn with_underline(mut self) -> Self {
        self.underline = UnderlineStyle::Single;
        self
    }

    pub fn with_strikethrough(mut self) -> Self {
        self.attrs |= AttrFlags::STRIKETHROUGH;
        self
    }

    pub fn with_blink(mut self) -> Self {
        self.attrs |= AttrFlags::SLOW_BLINK;
        self
    }

    pub fn with_rapid_blink(mut self) -> Self {
        self.attrs |= AttrFlags::RAPID_BLINK;
        self
    }

    pub fn with_reverse(mut self) -> Self {
        self.attrs |= AttrFlags::REVERSE;
        self
    }

    pub fn with_conceal(mut self) -> Self {
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

    /// Replace the entire attribute flag set.
    pub fn with_attrs(mut self, attrs: AttrFlags) -> Self {
        self.attrs = attrs;
        self
    }

    /// Attach a hyperlink to this style. Pass an empty `url` to clear
    /// the link; `params` are OSC 8 parameter pairs (e.g. `"id=foo"`)
    /// or empty.
    pub fn with_link(mut self, url: impl Into<String>, params: impl Into<String>) -> Self {
        let url = url.into();
        if url.is_empty() {
            self.link = None;
        } else {
            self.link = Some(Rc::new(LinkData {
                url,
                params: params.into(),
            }));
        }
        self
    }

    /// The attached hyperlink as `(url, params)`, or `None` when no
    /// link is set. `params` is empty when the link has none.
    pub fn link(&self) -> Option<(&str, &str)> {
        self.link
            .as_deref()
            .map(|l| (l.url.as_str(), l.params.as_str()))
    }

    /// Foreground color, if any.
    pub fn fg(&self) -> Option<Color> {
        self.fg
    }

    /// Background color, if any.
    pub fn bg(&self) -> Option<Color> {
        self.bg
    }

    /// Underline color, if any.
    pub fn underline_color(&self) -> Option<Color> {
        self.underline_color
    }

    /// Underline style.
    pub fn underline_style(&self) -> UnderlineStyle {
        self.underline
    }

    /// Text attribute flags.
    pub fn attrs(&self) -> AttrFlags {
        self.attrs
    }

    pub fn is_bold(&self) -> bool {
        self.attrs.contains(AttrFlags::BOLD)
    }

    pub fn is_faint(&self) -> bool {
        self.attrs.contains(AttrFlags::FAINT)
    }

    pub fn is_italic(&self) -> bool {
        self.attrs.contains(AttrFlags::ITALIC)
    }

    pub fn is_underlined(&self) -> bool {
        self.underline != UnderlineStyle::None
    }

    pub fn is_strikethrough(&self) -> bool {
        self.attrs.contains(AttrFlags::STRIKETHROUGH)
    }

    pub fn is_blinking(&self) -> bool {
        self.attrs.contains(AttrFlags::SLOW_BLINK)
    }

    pub fn is_rapid_blinking(&self) -> bool {
        self.attrs.contains(AttrFlags::RAPID_BLINK)
    }

    pub fn is_reversed(&self) -> bool {
        self.attrs.contains(AttrFlags::REVERSE)
    }

    pub fn is_concealed(&self) -> bool {
        self.attrs.contains(AttrFlags::CONCEAL)
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
            .with_bold()
            .with_italic()
            .with_fg(Color::Basic(BasicColor::Red));
        assert!(s.attrs.contains(AttrFlags::BOLD));
        assert!(s.attrs.contains(AttrFlags::ITALIC));
        assert_eq!(s.fg, Some(Color::Basic(BasicColor::Red)));
        assert!(!s.is_empty());
    }

    #[test]
    fn with_link_empty_url_clears() {
        let s = Style::EMPTY.with_link("https://x", "id=42");
        assert_eq!(s.link(), Some(("https://x", "id=42")));

        // Empty url clears, regardless of params.
        let s = s.with_link("", "id=ignored");
        assert!(s.link().is_none());
        assert!(s.is_empty());

        let s = Style::EMPTY.with_link("", "");
        assert!(s.link().is_none());
    }
}

//! Text style representation and SGR helpers.
//!
//! This module defines [`Style`], attribute flags, underline variants, and optional hyperlink targets for cells.
//! Reach for it when building styled text or when parsing, diffing, or emitting style changes.

pub mod diff;
pub mod parse;
pub mod sgr;

pub use diff::*;
pub use parse::*;
pub use sgr::*;

use std::io::{self, Write};
use std::sync::Arc;

use bitflags::bitflags;

use crate::color::Color;

/// Hyperlink target carried by a [`Style`]. Stored behind an [`Arc`]
/// so cells in a hyperlink span share a single allocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Link {
    pub url: String,
    pub params: String,
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
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub underline_color: Option<Color>,
    pub underline: UnderlineStyle,
    pub attrs: AttrFlags,
    pub link: Option<Arc<Link>>,
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
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
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

    pub fn blink(mut self) -> Self {
        self.attrs |= AttrFlags::SLOW_BLINK;
        self
    }

    pub fn rapid_blink(mut self) -> Self {
        self.attrs |= AttrFlags::RAPID_BLINK;
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

    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    pub fn underline_color(mut self, color: Color) -> Self {
        self.underline_color = Some(color);
        self
    }

    pub fn underline_style(mut self, style: UnderlineStyle) -> Self {
        self.underline = style;
        self
    }

    /// Replace the entire attribute flag set.
    pub fn attrs(mut self, attrs: AttrFlags) -> Self {
        self.attrs = attrs;
        self
    }

    /// Attach a hyperlink to this style. Pass an empty `url` to clear
    /// the link; `params` are OSC 8 parameter pairs (e.g. `"id=foo"`)
    /// or empty.
    pub fn link(mut self, url: impl Into<String>, params: impl Into<String>) -> Self {
        let url = url.into();
        if url.is_empty() {
            self.link = None;
        } else {
            self.link = Some(Arc::new(Link {
                url,
                params: params.into(),
            }));
        }
        self
    }

    /// Write this style's SGR sequence (`CSI ... m`) to `w`. An empty
    /// style writes the reset sequence (`CSI m`). Does not reset
    /// afterwards — following output stays in this style until changed.
    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        sgr::write_style(w, self)
    }

    /// Write `text` wrapped in this style: the style's SGR sequence,
    /// then `text`, then an SGR reset (`CSI m`) so subsequent output is
    /// unstyled.
    pub fn write_styled<W: Write>(&self, w: &mut W, text: &str) -> io::Result<()> {
        sgr::write_style(w, self)?;
        w.write_all(text.as_bytes())?;
        w.write_all(sgr::RESET)
    }

    /// Wrap `text` in this style for `Display`. The returned adapter
    /// renders the style's SGR sequence, the text, then an SGR reset,
    /// so it composes directly with `format!`/`write!` without a
    /// throwaway buffer. The `Display`-friendly companion to
    /// [`Style::write_styled`].
    pub fn styled<'a>(&self, text: &'a str) -> StyledText<'a> {
        StyledText {
            style: self.clone(),
            text,
        }
    }
}

/// Renders this style's SGR sequence (`CSI ... m`) — the opener only,
/// with no trailing reset, so it can be used as a standalone token. An
/// empty style renders the reset sequence (`CSI m`). For a wrapped span
/// (opener, text, reset) use [`Style::styled`].
impl std::fmt::Display for Style {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // SGR sequences are pure ASCII, so the bytes are valid UTF-8.
        let mut buf = Vec::new();
        self.write(&mut buf).map_err(|_| std::fmt::Error)?;
        let s = std::str::from_utf8(&buf).map_err(|_| std::fmt::Error)?;
        f.write_str(s)
    }
}

/// A piece of text bound to a [`Style`], renderable via [`Display`].
///
/// Created by [`Style::styled`]. Emitting it writes the style's SGR
/// sequence, the text, then an SGR reset.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug, Clone)]
pub struct StyledText<'a> {
    style: Style,
    text: &'a str,
}

impl std::fmt::Display for StyledText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // SGR sequences are ASCII and `text` is UTF-8, so the rendered
        // bytes are always valid UTF-8.
        let mut buf = Vec::new();
        self.style
            .write_styled(&mut buf, self.text)
            .map_err(|_| std::fmt::Error)?;
        let s = std::str::from_utf8(&buf).map_err(|_| std::fmt::Error)?;
        f.write_str(s)
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
            .fg(Color::Basic(BasicColor::Red));
        assert!(s.attrs.contains(AttrFlags::BOLD));
        assert!(s.attrs.contains(AttrFlags::ITALIC));
        assert_eq!(s.fg, Some(Color::Basic(BasicColor::Red)));
        assert!(!s.is_empty());
    }

    #[test]
    fn link_empty_url_clears() {
        let s = Style::EMPTY.link("https://x", "id=42");
        let l = s.link.as_deref().unwrap();
        assert_eq!((l.url.as_str(), l.params.as_str()), ("https://x", "id=42"));

        // Empty url clears, regardless of params.
        let s = s.link("", "id=ignored");
        assert!(s.link.is_none());
        assert!(s.is_empty());

        let s = Style::EMPTY.link("", "");
        assert!(s.link.is_none());
    }

    #[test]
    fn write_emits_sgr_without_reset() {
        let mut buf = Vec::new();
        Style::EMPTY
            .bold()
            .fg(Color::Basic(BasicColor::Red))
            .write(&mut buf)
            .unwrap();
        assert_eq!(buf, b"\x1b[1;31m");
    }

    #[test]
    fn write_empty_style_emits_reset() {
        let mut buf = Vec::new();
        Style::EMPTY.write(&mut buf).unwrap();
        assert_eq!(buf, b"\x1b[m");
    }

    #[test]
    fn write_styled_wraps_text_in_style_and_reset() {
        let mut buf = Vec::new();
        Style::EMPTY.bold().write_styled(&mut buf, "hi").unwrap();
        assert_eq!(buf, b"\x1b[1mhi\x1b[m");
    }

    #[test]
    fn styled_display_matches_write_styled() {
        let style = Style::EMPTY.bold();
        assert_eq!(format!("{}", style.styled("hi")), "\x1b[1mhi\x1b[m");
    }

    #[test]
    fn display_is_the_opener_only() {
        assert_eq!(Style::EMPTY.bold().to_string(), "\x1b[1m");
        // An empty style yields the reset sequence.
        assert_eq!(Style::EMPTY.to_string(), "\x1b[m");
    }
}

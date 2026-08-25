//! The style a cell is painted with: SGR appearance plus an optional
//! hyperlink.
//!
//! SGR and OSC 8 are independent terminal state machines, and a
//! [`Cell`](crate::cell::Cell) stores them independently. Authoring code
//! almost always decides both at once, though, so this is the type the
//! painting APIs take and the one that renders a complete opener and closer.
//!
//! The SGR half is [`style::Style`](crate::style::Style), aliased here as
//! `Sgr` so the two names can coexist in this file.

use std::borrow::Borrow;
use std::io::{self, Write};
use std::sync::Arc;

use crate::color::Color;
use crate::style::{AttrFlags, Link, Style as Sgr, UnderlineStyle};

/// SGR appearance paired with an optional hyperlink: everything needed to
/// paint a span of text.
///
/// Every [`style::Style`](crate::style::Style) builder is available here and
/// forwards to the SGR half, so a style can be composed in one chain and
/// finished with [`link`](Self::link). A plain SGR value also converts in,
/// so styling-only calls can pass one directly.
///
/// ```rust
/// use uncurses::cell::Style;
/// use uncurses::color::Color;
///
/// let heading = Style::new().bold().fg(Color::Green);
/// assert!(heading.link.is_none());
///
/// let docs = Style::new()
///     .underline()
///     .link("https://example.com", "");
/// assert_eq!(
///     docs.link.as_ref().map(|l| l.url.as_str()),
///     Some("https://example.com"),
/// );
///
/// // `{style}` writes the opener, `{style:#}` the matching closer. Unlike
/// // the SGR half, both halves cover the hyperlink too.
/// assert_eq!(format!("{docs}hi{docs:#}"), "\x1b[4m\x1b]8;;https://example.com\x1b\\hi\x1b]8;;\x1b\\\x1b[m");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Style {
    /// SGR appearance for the span.
    pub style: Sgr,
    /// OSC 8 hyperlink for the span, if any.
    ///
    /// Shared rather than owned so a run of identically linked cells costs
    /// one allocation between them instead of one each.
    pub link: Option<Arc<Link>>,
}

impl Style {
    /// Style with no colors, attributes, underline, or hyperlink.
    ///
    /// Writing it as an opener emits nothing, since the opener is additive.
    /// Format any style with the alternate flag (`{style:#}`) to emit an
    /// explicit return to the terminal default.
    pub const EMPTY: Style = Style {
        style: Sgr::EMPTY,
        link: None,
    };

    /// Create an empty style.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub const fn new() -> Self {
        Self::EMPTY
    }

    /// Return whether this style is entirely empty.
    ///
    /// Empty means no SGR-relevant fields and no OSC 8 hyperlink, which is
    /// equivalent to `*self == Style::EMPTY`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn is_empty(&self) -> bool {
        self.style.is_empty() && self.link.is_none()
    }

    /// Fill unset fields from `base` and return the result.
    ///
    /// Fields set on `self` win; unset ones are taken from `base`, and
    /// attributes are combined. Inheriting from any `base` therefore never
    /// clears something `self` already set. The hyperlink follows the same
    /// rule: `self`'s link wins, and `base`'s is used only when `self` has
    /// none.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn inherit(&self, base: impl Borrow<Style>) -> Style {
        let base = base.borrow();
        Style {
            style: self.style.inherit(base.style),
            link: self.link.clone().or_else(|| base.link.clone()),
        }
    }

    /// Attach an OSC 8 hyperlink and return the updated style.
    ///
    /// # Parameters
    ///
    /// - `url`: target URI. An empty URL leaves the style unlinked.
    /// - `params`: OSC 8 parameters, commonly `""` or `id=…`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn link(mut self, url: impl Into<String>, params: impl Into<String>) -> Self {
        let url = url.into();
        self.link = if url.is_empty() {
            None
        } else {
            Some(Arc::new(Link {
                url,
                params: params.into(),
            }))
        };
        self
    }

    /// Add bold intensity and return the updated style.
    pub fn bold(mut self) -> Self {
        self.style = self.style.bold();
        self
    }

    /// Add faint intensity and return the updated style.
    pub fn faint(mut self) -> Self {
        self.style = self.style.faint();
        self
    }

    /// Add italics and return the updated style.
    pub fn italic(mut self) -> Self {
        self.style = self.style.italic();
        self
    }

    /// Add a single underline and return the updated style.
    pub fn underline(mut self) -> Self {
        self.style = self.style.underline();
        self
    }

    /// Add a strikethrough and return the updated style.
    pub fn strikethrough(mut self) -> Self {
        self.style = self.style.strikethrough();
        self
    }

    /// Add slow blink and return the updated style.
    pub fn blink(mut self) -> Self {
        self.style = self.style.blink();
        self
    }

    /// Add rapid blink and return the updated style.
    pub fn rapid_blink(mut self) -> Self {
        self.style = self.style.rapid_blink();
        self
    }

    /// Swap foreground and background and return the updated style.
    pub fn reverse(mut self) -> Self {
        self.style = self.style.reverse();
        self
    }

    /// Conceal the text and return the updated style.
    pub fn conceal(mut self) -> Self {
        self.style = self.style.conceal();
        self
    }

    /// Set the foreground color and return the updated style.
    pub fn fg(mut self, color: impl Into<Option<Color>>) -> Self {
        self.style = self.style.fg(color);
        self
    }

    /// Set the background color and return the updated style.
    pub fn bg(mut self, color: impl Into<Option<Color>>) -> Self {
        self.style = self.style.bg(color);
        self
    }

    /// Set the underline color and return the updated style.
    pub fn underline_color(mut self, color: impl Into<Option<Color>>) -> Self {
        self.style = self.style.underline_color(color);
        self
    }

    /// Set the underline shape and return the updated style.
    pub fn underline_style(mut self, style: UnderlineStyle) -> Self {
        self.style = self.style.underline_style(style);
        self
    }

    /// Set the boolean SGR attributes and return the updated style.
    pub fn attrs(mut self, attrs: AttrFlags) -> Self {
        self.style = self.style.attrs(attrs);
        self
    }

    /// Write this style's opener bytes: the SGR sequence (`CSI … m`)
    /// followed by an OSC 8 hyperlink start when a link is attached.
    ///
    /// Additive: an [empty](Self::is_empty) style writes nothing.
    fn write_opener<W: Write>(&self, w: &mut W) -> io::Result<()> {
        write!(w, "{}", self.style)?;
        if let Some(link) = &self.link {
            crate::ansi::hyperlink::write_hyperlink(w, &link.url, &link.params)?;
        }
        Ok(())
    }

    /// Write this style's closer bytes: the OSC 8 terminator when a link is
    /// attached, followed by the SGR reset (`CSI m`) when any SGR state is
    /// set.
    ///
    /// The SGR reset clears all attributes to their defaults, so the closer
    /// returns the terminal to a clean state rather than restoring whatever
    /// style was active before the opener. An [empty](Self::is_empty) style
    /// writes nothing.
    fn write_closer<W: Write>(&self, w: &mut W) -> io::Result<()> {
        if self.link.is_some() {
            w.write_all(crate::ansi::hyperlink::HYPERLINK_RESET)?;
        }
        write!(w, "{:#}", self.style)?;
        Ok(())
    }
}

/// Render this style as ANSI escape sequences.
///
/// The default form (`{style}`) renders the **opener**: the SGR sequence
/// followed by an OSC 8 hyperlink start when a link is attached. The opener
/// is additive, so an empty style renders nothing.
///
/// The alternate form (`{style:#}`) renders the **closer**: the OSC 8
/// terminator followed by the SGR reset.
///
/// ```
/// use uncurses::cell::Style;
/// use uncurses::color::Color;
///
/// let style = Style::new().bold().fg(Color::Green);
/// assert_eq!(format!("{style}hi{style:#}"), "\x1b[1;32mhi\x1b[m");
/// ```
impl std::fmt::Display for Style {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // SGR sequences and OSC 8 framing are pure ASCII, so the bytes are
        // valid UTF-8.
        let mut buf = Vec::new();
        if f.alternate() {
            self.write_closer(&mut buf).map_err(|_| std::fmt::Error)?;
        } else {
            self.write_opener(&mut buf).map_err(|_| std::fmt::Error)?;
        }
        let s = std::str::from_utf8(&buf).map_err(|_| std::fmt::Error)?;
        f.write_str(s)
    }
}

impl From<Sgr> for Style {
    fn from(style: Sgr) -> Self {
        Style { style, link: None }
    }
}

impl From<&Sgr> for Style {
    fn from(style: &Sgr) -> Self {
        Style::from(*style)
    }
}

impl From<&Style> for Style {
    fn from(style: &Style) -> Self {
        style.clone()
    }
}

impl From<Option<Sgr>> for Style {
    /// `None` paints with the default style and no hyperlink, mirroring
    /// `From<Option<Sgr>> for Sgr`.
    fn from(style: Option<Sgr>) -> Self {
        Style::from(style.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_forward_to_the_sgr_half() {
        let s = Style::new().bold().fg(Color::Green).underline();
        assert!(s.style.attrs.contains(AttrFlags::BOLD));
        assert_eq!(s.style.fg, Some(Color::Green));
        assert_eq!(s.style.underline, UnderlineStyle::Single);
        assert!(s.link.is_none());
    }

    #[test]
    fn empty_style_renders_nothing_either_way() {
        let s = Style::new();
        assert!(s.is_empty());
        assert_eq!(format!("{s}"), "");
        assert_eq!(format!("{s:#}"), "");
    }

    #[test]
    fn opener_puts_the_hyperlink_after_the_sgr() {
        let s = Style::new().bold().link("https://example.com", "id=1");
        let opened = format!("{s}");
        let sgr_end = opened.find('m').expect("sgr opener");
        let osc = opened.find("\x1b]8;").expect("osc 8 opener");
        assert!(sgr_end < osc, "sgr must precede osc 8: {opened:?}");
        assert!(opened.contains("id=1"));
    }

    #[test]
    fn closer_closes_the_link_then_resets_sgr() {
        let s = Style::new().bold().link("https://example.com", "");
        let closed = format!("{s:#}");
        let osc = closed.find("\x1b]8;;").expect("osc 8 terminator");
        let reset = closed.find("\x1b[m").expect("sgr reset");
        assert!(osc < reset, "link closes before the sgr reset: {closed:?}");
    }

    #[test]
    fn a_link_only_style_resets_only_the_link() {
        let s = Style::new().link("https://example.com", "");
        assert!(!s.is_empty());
        assert_eq!(format!("{s:#}"), "\x1b]8;;\x1b\\");
    }

    #[test]
    fn link_with_an_empty_url_clears_it() {
        let s = Style::new().link("https://example.com", "").link("", "");
        assert!(s.link.is_none());
    }

    #[test]
    fn inherit_prefers_self_and_fills_from_base() {
        let base = Style::new().fg(Color::Red).link("https://base", "");
        let own = Style::new().bold();
        let merged = own.inherit(&base);
        assert!(merged.style.attrs.contains(AttrFlags::BOLD));
        assert_eq!(merged.style.fg, Some(Color::Red));
        assert_eq!(
            merged.link.as_ref().map(|l| l.url.as_str()),
            Some("https://base")
        );

        let own_link = Style::new().link("https://own", "");
        assert_eq!(
            own_link
                .inherit(&base)
                .link
                .as_ref()
                .map(|l| l.url.as_str()),
            Some("https://own"),
        );
    }

    #[test]
    fn an_sgr_value_converts_in_unlinked() {
        let s: Style = Sgr::new().bold().into();
        assert!(s.style.attrs.contains(AttrFlags::BOLD));
        assert!(s.link.is_none());
    }
}

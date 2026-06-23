//! Text style values and terminal SGR/OSC 8 rendering.
//!
//! ## Style as a value
//!
//! [`Style`] is an owned description of how text should look: optional
//! foreground/background colors, optional underline color, an
//! [`UnderlineStyle`], an [`AttrFlags`] bitset, and an optional OSC 8
//! [`Link`]. Builder methods take and return `Self`, so styles can be composed
//! fluently and then cloned into cells or spans.
//!
//! ## Open/close versus wrapped rendering
//!
//! [`Style::write_style`] and the [`std::fmt::Display`] implementation for
//! [`Style`] emit the opener: the SGR sequence (`CSI … m`), followed by the
//! OSC 8 hyperlink start when the style carries a link. They do not reset the
//! terminal or close the link afterward; following output remains in that
//! style until another style or reset is written.
//!
//! [`Style::write_styled`] and [`Style::styled`] render a complete span: the
//! opener, the text, then the matching close. The hyperlink and SGR state are
//! closed in reverse order of opening, so the opener and the wrapped form stay
//! symmetric:
//!
//! ```text
//! without link: ┌─────────┐ ┌──────┐ ┌───────┐
//!               │ CSI … m │▶│ text │▶│ CSI m │
//!               └─────────┘ └──────┘ └───────┘
//! with link:    ┌─────────┐ ┌───────┐ ┌──────┐ ┌───────────┐ ┌───────┐
//!               │ CSI … m │▶│ OSC 8 │▶│ text │▶│ OSC 8 end │▶│ CSI m │
//!               └─────────┘ └───────┘ └──────┘ └───────────┘ └───────┘
//! ```
//!
//! ## Attributes and underline
//!
//! Boolean SGR attributes such as bold, italic, blinking, reverse video,
//! conceal, and strikethrough live in [`AttrFlags`]. Underlining is modeled
//! separately with [`UnderlineStyle`] because SGR supports multiple underline
//! shapes (`4`, `4:2`, `4:3`, `4:4`, `4:5`) and an independent underline
//! color.
//!
//! ## SGR encoding
//!
//! Style emission uses a single `CSI … m` sequence for all SGR state. Standard
//! foreground/background colors use `30`–`37`/`40`–`47`, bright colors use
//! `90`–`97`/`100`–`107`, indexed colors use `38;5;n`/`48;5;n`, true color
//! uses `38;2;r;g;b`/`48;2;r;g;b`, and underline color uses the colon
//! subparameter form (`58:5:n` or `58:2::r:g:b`).
//!
//! ```text
//! ESC [   1 ;   4:3  ;    38;2;255;128;0  ; 58:2::0:255:255     m
//! └─┬─┘ └─────────────── SGR parameters ─────────────────────┘ └┬┘
//!  CSI  attrs  underline   fg truecolor     ul color          final
//! ```
//!
//! ```rust,ignore
//! use uncurses::color::Color;
//! use uncurses::style::Style;
//!
//! let heading = Style::default().bold().fg(Color::Green);
//! let mut out = Vec::new();
//! heading.write_styled(&mut out, "Hello")?;
//!
//! let link = Style::default()
//!     .underline()
//!     .link("https://example.com", "");
//! println!("{}", link.styled("docs"));
//! # Ok::<(), std::io::Error>(())
//! ```

pub(crate) mod diff;
mod parse;
mod sgr;

pub(crate) use parse::read_style;
#[cfg(test)]
pub(crate) use sgr::RESET;

use std::borrow::Borrow;
use std::io::{self, Write};
use std::sync::Arc;

use bitflags::bitflags;

use crate::color::Color;

/// OSC 8 hyperlink target carried by a [`Style`].
///
/// A link is used when styled text should also open a terminal hyperlink.
/// [`Style::link`] stores non-empty URLs behind an [`Arc`] so many cells in
/// the same hyperlink span can share one allocation. The URL and parameter
/// string are emitted verbatim as `OSC 8 ; params ; url ST`; callers are
/// responsible for passing values that are appropriate for the target
/// terminal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Link {
    /// Target URI written into the OSC 8 sequence.
    ///
    /// Empty URLs are not stored by [`Style::link`]; passing an empty URL to
    /// that builder clears the current link instead.
    pub url: String,
    /// OSC 8 parameter string written between the two semicolons.
    ///
    /// Use an empty string for no parameters, or terminal-supported
    /// `key=value` pairs such as `id=section`.
    pub params: String,
}

bitflags! {
    /// Bitflags for SGR text attributes.
    ///
    /// These are the boolean attributes that can be combined freely on a
    /// [`Style`]. Underline shape is tracked separately by
    /// [`UnderlineStyle`], and colors are stored as [`Color`](crate::color::Color).
    /// Use [`Style::attrs`] when replacing the whole set, or the convenience
    /// builders such as [`Style::bold`] and [`Style::italic`] when adding one
    /// flag at a time.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct AttrFlags: u16 {
        /// Bold/intense text (`SGR 1`).
        ///
        /// Cleared together with [`FAINT`](Self::FAINT) by `SGR 22`.
        const BOLD          = 0b0000_0000_0001;
        /// Faint/decreased-intensity text (`SGR 2`).
        ///
        /// Cleared together with [`BOLD`](Self::BOLD) by `SGR 22`.
        const FAINT         = 0b0000_0000_0010;
        /// Italic text (`SGR 3`, cleared by `SGR 23`).
        const ITALIC        = 0b0000_0000_0100;
        /// Slow blinking text (`SGR 5`).
        ///
        /// Cleared together with [`RAPID_BLINK`](Self::RAPID_BLINK) by
        /// `SGR 25`.
        const SLOW_BLINK    = 0b0000_0000_1000;
        /// Rapid blinking text (`SGR 6`).
        ///
        /// Cleared together with [`SLOW_BLINK`](Self::SLOW_BLINK) by
        /// `SGR 25`.
        const RAPID_BLINK   = 0b0000_0001_0000;
        /// Reverse foreground and background (`SGR 7`, cleared by `SGR 27`).
        const REVERSE       = 0b0000_0010_0000;
        /// Concealed text (`SGR 8`, cleared by `SGR 28`).
        const CONCEAL       = 0b0000_0100_0000;
        /// Struck-through text (`SGR 9`, cleared by `SGR 29`).
        const STRIKETHROUGH = 0b0000_1000_0000;
    }
}

/// Underline shape encoded in SGR underline parameters.
///
/// Use [`Style::underline`] for the common single underline or
/// [`Style::underline_style`] to select an explicit shape. [`None`](Self::None)
/// means no underline; it emits no parameter when writing a full [`Style`] and
/// emits `SGR 24` when a diff needs to clear an existing underline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum UnderlineStyle {
    #[default]
    /// No underline.
    None = 0,
    /// Single underline (`SGR 4` or `4:1` when parsed).
    Single = 1,
    /// Double underline (`SGR 4:2`; `SGR 21` is parsed as this variant).
    Double = 2,
    /// Curly underline (`SGR 4:3`).
    Curly = 3,
    /// Dotted underline (`SGR 4:4`).
    Dotted = 4,
    /// Dashed underline (`SGR 4:5`).
    Dashed = 5,
}

/// A complete terminal text style.
///
/// `Style` is the central value used by cells, text painters, and renderers.
/// It stores SGR state (foreground/background/underline colors, underline
/// shape, and attributes) plus an optional OSC 8 hyperlink. Build styles with
/// the provided value-taking builders, then either emit the opener with
/// [`Style::write_style`] / [`std::fmt::Display`] or wrap text with
/// [`Style::write_styled`] / [`Style::styled`].
///
/// Cloning is cheap for hyperlinks: the [`Link`] is reference-counted so a
/// long span of identically-linked cells keeps a single shared allocation.
#[derive(Debug, Clone, Default)]
pub struct Style {
    /// Foreground/text color.
    ///
    /// `None` leaves the terminal's current/default foreground unchanged when
    /// writing a full style; diffs use `SGR 39` to clear a previous foreground.
    pub fg: Option<Color>,
    /// Background color.
    ///
    /// `None` leaves the terminal's current/default background unchanged when
    /// writing a full style; diffs use `SGR 49` to clear a previous background.
    pub bg: Option<Color>,
    /// Underline color.
    ///
    /// Encoded with SGR `58` when present. `None` means use the terminal's
    /// default underline color; diffs use `SGR 59` to clear a previous value.
    pub underline_color: Option<Color>,
    /// Underline shape.
    pub underline: UnderlineStyle,
    /// Boolean SGR text attributes.
    pub attrs: AttrFlags,
    /// Optional OSC 8 hyperlink target.
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

impl From<&Style> for Style {
    /// Clone a borrowed style into an owned one.
    ///
    /// This lets APIs that accept `impl Into<Style>` take a `&Style` without
    /// the caller writing `.clone()`. Owned styles convert for free via the
    /// blanket `From<Style> for Style`.
    fn from(style: &Style) -> Self {
        style.clone()
    }
}

impl From<Option<Style>> for Style {
    /// Convert an optional style, mapping `None` to [`Style::EMPTY`].
    ///
    /// This lets APIs that accept `impl Into<Style>` take a bare `None` to mean
    /// "no styling", e.g. `surface.set_str(pos, text, None)`. `Some(style)`
    /// unwraps to the contained style.
    fn from(style: Option<Style>) -> Self {
        style.unwrap_or(Style::EMPTY)
    }
}

impl Style {
    /// Create an empty style.
    ///
    /// Equivalent to [`Style::default`] and [`Style::EMPTY`]: no colors,
    /// attributes, underline, or hyperlink. Chain the builder methods such as
    /// [`bold`](Self::bold) or [`fg`](Self::fg) to add settings.
    pub const fn new() -> Self {
        Self::EMPTY
    }

    /// Style with no colors, attributes, underline, or hyperlink.
    ///
    /// This is equivalent to [`Style::default()`]. Writing it as a style opener
    /// emits nothing, since the opener is additive; use [`Style::reset`] to
    /// emit an explicit return to the terminal default.
    pub const EMPTY: Style = Style {
        fg: None,
        bg: None,
        underline_color: None,
        underline: UnderlineStyle::None,
        attrs: AttrFlags::empty(),
        link: None,
    };

    /// Return whether this style is entirely empty.
    ///
    /// Empty means no SGR-relevant fields and no OSC 8 hyperlink. This is
    /// equivalent to `*self == Style::EMPTY`.
    pub fn is_empty(&self) -> bool {
        self.is_sgr_empty() && self.is_link_empty()
    }

    /// Return whether this style has no SGR-relevant settings.
    ///
    /// Colors, attributes, underline shape, and underline color are checked.
    /// The hyperlink is intentionally ignored because it is emitted via OSC 8,
    /// not SGR.
    pub(crate) fn is_sgr_empty(&self) -> bool {
        self.fg.is_none()
            && self.bg.is_none()
            && self.underline_color.is_none()
            && self.underline == UnderlineStyle::None
            && self.attrs.is_empty()
    }

    /// Return whether this style carries no hyperlink.
    ///
    /// Companion to [`Style::is_sgr_empty`]; together they decide whether the
    /// style value has any terminal-visible state.
    pub(crate) fn is_link_empty(&self) -> bool {
        self.link.is_none()
    }

    /// Inherit from `base`, returning `self` with its unset fields filled in.
    ///
    /// `self` takes precedence, like a child overriding inherited values: its
    /// foreground, background, underline color, underline shape, and hyperlink
    /// win wherever `self` sets them, and `base` only supplies a fallback for
    /// each field `self` leaves at its default. Attributes from both are
    /// combined. Inheriting from any `base` therefore never clears a field
    /// `self` already set, and inheriting from an empty `base` returns `self`
    /// unchanged.
    ///
    /// `base` is borrowed, so either an owned [`Style`] or a `&Style` may be
    /// passed without cloning.
    pub fn inherit(&self, base: impl Borrow<Style>) -> Style {
        let base = base.borrow();
        Style {
            fg: self.fg.or(base.fg),
            bg: self.bg.or(base.bg),
            underline_color: self.underline_color.or(base.underline_color),
            underline: if self.underline == UnderlineStyle::None {
                base.underline
            } else {
                self.underline
            },
            attrs: self.attrs | base.attrs,
            link: self.link.clone().or_else(|| base.link.clone()),
        }
    }

    /// Add bold intensity and return the updated style.
    ///
    /// This sets [`AttrFlags::BOLD`] and leaves all other fields unchanged.
    pub fn bold(mut self) -> Self {
        self.attrs |= AttrFlags::BOLD;
        self
    }

    /// Add faint intensity and return the updated style.
    ///
    /// This sets [`AttrFlags::FAINT`] and leaves all other fields unchanged.
    pub fn faint(mut self) -> Self {
        self.attrs |= AttrFlags::FAINT;
        self
    }

    /// Add italic text and return the updated style.
    ///
    /// This sets [`AttrFlags::ITALIC`] and leaves all other fields unchanged.
    pub fn italic(mut self) -> Self {
        self.attrs |= AttrFlags::ITALIC;
        self
    }

    /// Use a single underline and return the updated style.
    ///
    /// Equivalent to [`Style::underline_style`] with
    /// [`UnderlineStyle::Single`].
    pub fn underline(mut self) -> Self {
        self.underline = UnderlineStyle::Single;
        self
    }

    /// Add strikethrough text and return the updated style.
    ///
    /// This sets [`AttrFlags::STRIKETHROUGH`] and leaves all other fields
    /// unchanged.
    pub fn strikethrough(mut self) -> Self {
        self.attrs |= AttrFlags::STRIKETHROUGH;
        self
    }

    /// Add slow blinking text and return the updated style.
    ///
    /// This sets [`AttrFlags::SLOW_BLINK`] and leaves all other fields
    /// unchanged.
    pub fn blink(mut self) -> Self {
        self.attrs |= AttrFlags::SLOW_BLINK;
        self
    }

    /// Add rapid blinking text and return the updated style.
    ///
    /// This sets [`AttrFlags::RAPID_BLINK`] and leaves all other fields
    /// unchanged.
    pub fn rapid_blink(mut self) -> Self {
        self.attrs |= AttrFlags::RAPID_BLINK;
        self
    }

    /// Reverse foreground and background and return the updated style.
    ///
    /// This sets [`AttrFlags::REVERSE`]; it does not swap the stored `fg` and
    /// `bg` values.
    pub fn reverse(mut self) -> Self {
        self.attrs |= AttrFlags::REVERSE;
        self
    }

    /// Add concealed text and return the updated style.
    ///
    /// This sets [`AttrFlags::CONCEAL`] and leaves all other fields unchanged.
    pub fn conceal(mut self) -> Self {
        self.attrs |= AttrFlags::CONCEAL;
        self
    }

    /// Set or clear the foreground color and return the updated style.
    ///
    /// Accepts any value convertible into `Option<Color>`, including a
    /// [`Color`] or `None`.
    /// Passing `None` clears any foreground color carried by the base style.
    pub fn fg(mut self, color: impl Into<Option<Color>>) -> Self {
        self.fg = color.into();
        self
    }

    /// Set or clear the background color and return the updated style.
    ///
    /// Accepts any value convertible into `Option<Color>`, including a
    /// [`Color`] or `None`.
    /// Passing `None` clears any background color carried by the base style.
    pub fn bg(mut self, color: impl Into<Option<Color>>) -> Self {
        self.bg = color.into();
        self
    }

    /// Set or clear the underline color and return the updated style.
    ///
    /// Accepts any value convertible into `Option<Color>`, including a
    /// [`Color`] or `None`.
    /// Passing `None` clears any underline color carried by the base style.
    pub fn underline_color(mut self, color: impl Into<Option<Color>>) -> Self {
        self.underline_color = color.into();
        self
    }

    /// Set the underline shape and return the updated style.
    ///
    /// Use [`UnderlineStyle::None`] to clear underlining.
    pub fn underline_style(mut self, style: UnderlineStyle) -> Self {
        self.underline = style;
        self
    }

    /// Replace the entire attribute flag set and return the updated style.
    ///
    /// This is useful when applying a previously computed [`AttrFlags`] value.
    /// Use the convenience builders when adding a single flag.
    pub fn attrs(mut self, attrs: AttrFlags) -> Self {
        self.attrs = attrs;
        self
    }

    /// Attach or clear an OSC 8 hyperlink and return the updated style.
    ///
    /// A non-empty `url` stores a [`Link`] with the supplied `params`. An empty
    /// `url` clears any existing link and ignores `params`. Parameters are the
    /// raw OSC 8 parameter string, commonly empty or a terminal-supported value
    /// such as `id=foo`.
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

    /// Write this style's opener to `w`: the SGR sequence (`CSI … m`)
    /// followed by an OSC 8 hyperlink start when this style carries a
    /// [`link`](Self::link).
    ///
    /// The opener is additive: it emits only the state this style sets, so an
    /// [empty](Self::is_empty) style writes nothing at all. It does not reset
    /// afterward, so following output stays in this style until changed; a
    /// caller that opens a hyperlink here is responsible for closing it.
    /// [`write_styled`](Self::write_styled) pairs this opener with the matching
    /// close, and [`write_reset`](Self::write_reset) emits that close on its
    /// own. Returns any I/O error from `w`; it does not panic.
    pub fn write_style<W: Write>(&self, w: &mut W) -> io::Result<()> {
        sgr::write_style(w, self)?;
        if let Some(link) = &self.link {
            crate::ansi::hyperlink::write_hyperlink(w, &link.url, &link.params)?;
        }
        Ok(())
    }

    /// Write this style's closer to `w`: the OSC 8 hyperlink terminator when
    /// this style carries a [`link`](Self::link), followed by the SGR reset
    /// (`CSI m`) when it carries any SGR state.
    ///
    /// This is the mirror of [`write_style`](Self::write_style): it closes
    /// exactly the
    /// state machines that the opener touched and in reverse order, so an
    /// [empty](Self::is_empty) style writes nothing at all. Closing state that
    /// is already clear would be a harmless no-op, but skipping it keeps the
    /// closer as compact as the opener. Returns any I/O error from `w`; it does
    /// not panic.
    pub fn write_reset<W: Write>(&self, w: &mut W) -> io::Result<()> {
        if self.link.is_some() {
            w.write_all(crate::ansi::hyperlink::HYPERLINK_RESET)?;
        }
        if !self.is_sgr_empty() {
            w.write_all(sgr::RESET)?;
        }
        Ok(())
    }

    /// Return a [`std::fmt::Display`] adapter that writes this style's closer.
    ///
    /// Formatting the returned [`Reset`] is equivalent to calling
    /// [`Style::write_reset`]: it emits the OSC 8 hyperlink terminator (when the
    /// style carries a link) followed by the SGR reset (when it carries SGR
    /// state). An [empty](Self::is_empty) style renders nothing. Use it as the
    /// closer when composing styled output through formatting macros:
    /// `format!("{open}text{}", open.reset())`.
    pub fn reset(&self) -> Reset {
        Reset {
            sgr: !self.is_sgr_empty(),
            link: self.link.is_some(),
        }
    }

    /// Write `text` as a complete styled span.
    ///
    /// This is the opener from [`write_style`](Self::write_style) followed by
    /// `text` and
    /// the matching close from [`write_reset`](Self::write_reset), so a span is
    /// self-contained. The emitted order is: SGR opener, optional OSC 8
    /// hyperlink start, `text` bytes, optional OSC 8 hyperlink terminator, then
    /// the SGR reset (`CSI m`). The close prevents the style from leaking into
    /// later output. Returns any I/O error from `w`; it does not panic.
    ///
    /// An [empty](Self::is_empty) style carries no terminal-visible state, so
    /// this writes `text` verbatim with no SGR or OSC 8 sequences at all.
    pub fn write_styled<W: Write>(&self, w: &mut W, text: &str) -> io::Result<()> {
        if self.is_empty() {
            return w.write_all(text.as_bytes());
        }
        self.write_style(w)?;
        w.write_all(text.as_bytes())?;
        self.write_reset(w)
    }

    /// Return a [`std::fmt::Display`] adapter that renders `text` as a span.
    ///
    /// Formatting the returned [`StyledText`] is equivalent to calling
    /// [`Style::write_styled`]: it includes the SGR opener, text, reset, and
    /// any OSC 8 hyperlink wrapper. An [empty](Self::is_empty) style renders
    /// `text` verbatim with no escape sequences. Use this with `format!`,
    /// `println!`, or `write!` when a `Display` value is more convenient than an
    /// [`std::io::Write`].
    pub fn styled<'a>(&self, text: &'a str) -> StyledText<'a> {
        StyledText {
            style: self.clone(),
            text,
        }
    }
}

/// Render this style's opener: the SGR sequence (`CSI … m`) followed by an
/// OSC 8 hyperlink start when this style carries a [`link`](Style::link).
///
/// This is the opener only: it does not include a trailing reset and does not
/// close the hyperlink. The opener is additive, so an empty style renders
/// nothing. For a complete span, use [`Style::styled`]; for the closer on its
/// own, use [`Style::reset`].
impl std::fmt::Display for Style {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // SGR sequences are pure ASCII, so the bytes are valid UTF-8.
        let mut buf = Vec::new();
        self.write_style(&mut buf).map_err(|_| std::fmt::Error)?;
        let s = std::str::from_utf8(&buf).map_err(|_| std::fmt::Error)?;
        f.write_str(s)
    }
}

/// A [`std::fmt::Display`] adapter that writes a style's closer.
///
/// Created by [`Style::reset`]. Formatting it emits the OSC 8 hyperlink
/// terminator (when the originating style carried a link) followed by the SGR
/// reset (`CSI m`) (when it carried SGR state), mirroring the opener it closes.
/// A reset for an empty style writes nothing. Use it as the closer in
/// formatting macros when you open a style manually rather than through
/// [`Style::styled`].
#[derive(Debug, Clone, Copy)]
pub struct Reset {
    sgr: bool,
    link: bool,
}

impl std::fmt::Display for Reset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Both the hyperlink terminator and the SGR reset are pure ASCII.
        if self.link {
            f.write_str(std::str::from_utf8(crate::ansi::hyperlink::HYPERLINK_RESET).unwrap())?;
        }
        if self.sgr {
            f.write_str(std::str::from_utf8(sgr::RESET).unwrap())?;
        }
        Ok(())
    }
}

/// A borrowed text span bound to an owned [`Style`].
///
/// Created by [`Style::styled`]. Formatting a non-empty style writes the SGR
/// opener, the optional OSC 8 hyperlink start, the borrowed text, then the full
/// close from [`Style::reset`] (OSC 8 terminator and SGR reset). An empty style
/// renders the text alone. Use it when composing styled text through formatting
/// macros; use [`Style::write_styled`] when writing to an [`std::io::Write`]
/// directly.
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
    use crate::color::Color;

    #[test]
    fn test_style_empty() {
        assert!(Style::EMPTY.is_empty());
        assert!(Style::default().is_empty());
    }

    #[test]
    fn test_style_builder() {
        let s = Style::EMPTY.bold().italic().fg(Color::Red);
        assert!(s.attrs.contains(AttrFlags::BOLD));
        assert!(s.attrs.contains(AttrFlags::ITALIC));
        assert_eq!(s.fg, Some(Color::Red));
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
            .fg(Color::Red)
            .write_style(&mut buf)
            .unwrap();
        assert_eq!(buf, b"\x1b[1;31m");
    }

    #[test]
    fn write_empty_style_emits_nothing() {
        let mut buf = Vec::new();
        Style::EMPTY.write_style(&mut buf).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn write_styled_wraps_text_in_style_and_reset() {
        let mut buf = Vec::new();
        Style::EMPTY.bold().write_styled(&mut buf, "hi").unwrap();
        // SGR-only style: opener, text, then just the SGR reset (no link close).
        assert_eq!(buf, b"\x1b[1mhi\x1b[m");
    }

    #[test]
    fn write_reset_closes_only_what_is_set() {
        // SGR-only style closes with the SGR reset alone.
        let mut buf = Vec::new();
        Style::EMPTY.bold().write_reset(&mut buf).unwrap();
        assert_eq!(buf, b"\x1b[m");

        // A link adds the OSC 8 terminator before the SGR reset.
        let mut buf = Vec::new();
        Style::EMPTY
            .bold()
            .link("https://x", "")
            .write_reset(&mut buf)
            .unwrap();
        assert_eq!(buf, b"\x1b]8;;\x1b\\\x1b[m");

        // A link-only style emits just the OSC 8 terminator, no SGR reset.
        let mut buf = Vec::new();
        Style::EMPTY
            .link("https://x", "")
            .write_reset(&mut buf)
            .unwrap();
        assert_eq!(buf, b"\x1b]8;;\x1b\\");

        // An empty style resets nothing.
        let mut buf = Vec::new();
        Style::EMPTY.write_reset(&mut buf).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn reset_display_matches_write_reset() {
        let style = Style::EMPTY.bold().link("https://x", "");
        let mut buf = Vec::new();
        style.write_reset(&mut buf).unwrap();
        assert_eq!(style.reset().to_string().as_bytes(), buf.as_slice());
    }

    #[test]
    fn write_styled_empty_style_emits_text_only() {
        let mut buf = Vec::new();
        Style::EMPTY.write_styled(&mut buf, "hi").unwrap();
        assert_eq!(buf, b"hi");
    }

    #[test]
    fn inherit_self_wins_and_fills_unset_from_base() {
        let style = Style::EMPTY.italic().fg(Color::Blue);
        let base = Style::EMPTY.bold().fg(Color::Red).bg(Color::Black);
        let merged = style.inherit(base);
        // self's fg wins; bg is inherited from base; attributes combine.
        assert_eq!(merged.fg, Some(crate::color::Color::Blue));
        assert_eq!(merged.bg, Some(crate::color::Color::Black));
        assert!(merged.attrs.contains(AttrFlags::BOLD));
        assert!(merged.attrs.contains(AttrFlags::ITALIC));
    }

    #[test]
    fn inherit_empty_self_returns_base() {
        let base = Style::EMPTY.bold().fg(Color::Red);
        // An empty child inherits everything from the base.
        assert_eq!(Style::EMPTY.inherit(&base), base);
    }

    #[test]
    fn inherit_empty_base_keeps_self() {
        let style = Style::EMPTY.bold().fg(Color::Red);
        let merged = style.inherit(Style::EMPTY);
        assert_eq!(merged, style);
    }

    #[test]
    fn styled_empty_style_displays_text_only() {
        assert_eq!(format!("{}", Style::EMPTY.styled("hi")), "hi");
    }

    #[test]
    fn write_styled_wraps_link_in_osc8() {
        let mut buf = Vec::new();
        Style::EMPTY
            .underline()
            .link("https://example.com", "")
            .write_styled(&mut buf, "docs")
            .unwrap();
        // SGR opener, hyperlink start, text, hyperlink end, SGR reset.
        assert_eq!(
            buf,
            b"\x1b[4m\x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x1b\\\x1b[m"
        );
    }

    #[test]
    fn write_opener_includes_hyperlink_after_sgr() {
        let mut buf = Vec::new();
        Style::EMPTY
            .underline()
            .link("https://example.com", "")
            .write_style(&mut buf)
            .unwrap();
        // The opener is the SGR sequence followed by the OSC 8 hyperlink start.
        assert_eq!(buf, b"\x1b[4m\x1b]8;;https://example.com\x1b\\");
    }

    #[test]
    fn styled_display_matches_write_styled() {
        let style = Style::EMPTY.bold();
        assert_eq!(format!("{}", style.styled("hi")), "\x1b[1mhi\x1b[m");
    }

    #[test]
    fn display_is_the_opener_only() {
        assert_eq!(Style::EMPTY.bold().to_string(), "\x1b[1m");
        // An empty style's opener emits nothing.
        assert_eq!(Style::EMPTY.to_string(), "");
    }
}

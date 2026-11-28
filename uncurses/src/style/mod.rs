//! Text style values and terminal SGR/OSC 8 rendering.
//!
//! ## Style as a value
//!
//! [`Style`] is an owned description of how text should look: optional
//! foreground/background colors, optional underline color, an
//! [`UnderlineStyle`], an [`AttrFlags`] bitset, and an optional OSC 8
//! [`Link`]. Builder methods take and return `Self`, so styles can be composed
//! fluently and then cloned into cells or spans.
//! Private fields keep the value consistent. Inspect components with
//! [`Style::fg_color`], [`Style::bg_color`], [`Style::ul_color`],
//! [`Style::attributes`], [`Style::underline_shape`], and [`Style::link_target`].
//!
//! ## Open/close versus wrapped rendering
//!
//! The [`std::fmt::Display`] implementation for [`Style`] emits the opener: the
//! SGR sequence (`CSI … m`), followed by the OSC 8 hyperlink start when the
//! style carries a link. The opener does not reset the terminal or close the
//! link afterward; following output remains in that style until another style
//! or reset is written.
//!
//! The alternate form (`{style:#}`) emits the matching closer: the OSC 8
//! hyperlink terminator (when the style carries a link) followed by the SGR
//! reset (`CSI m`). Used together, `{style}` and `{style:#}` wrap a complete
//! span with a single style value. The SGR reset clears to defaults rather than
//! restoring a previously active style, so wrap each span independently:
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
//! ```rust
//! use uncurses::color::Color;
//! use uncurses::style::Style;
//!
//! // `{style}` writes the opener; `{style:#}` writes the matching closer.
//! let heading = Style::default().bold().fg(Color::Green);
//! println!("{heading}Hello{heading:#}");
//!
//! let link = Style::default()
//!     .underline()
//!     .link("https://example.com", "");
//! println!("{link}docs{link:#}");
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
/// the provided value-taking builders, then render with the
/// [`std::fmt::Display`] implementation: `{style}` writes the opener and
/// `{style:#}` writes the matching closer.
///
/// Cloning is cheap for hyperlinks: the [`Link`] is reference-counted so a
/// long span of identically-linked cells keeps a single shared allocation.
/// Cell imports and owned snapshots retain the original [`Arc<Link>`].
#[derive(Debug, Clone, Default)]
pub struct Style {
    sgr: u64,
    // Keep an explicit underline color independent of foreground/background edits.
    underline_color: Option<Color>,
    link: Option<Arc<Link>>,
}

impl Style {
    pub(crate) fn from_packed_sgr(
        sgr: u64,
        underline_color: Option<Color>,
        link: Option<&Arc<Link>>,
    ) -> Self {
        Self {
            // Packed cells use the top bits for the underline color selector.
            sgr: sgr & ((1 << 61) - 1),
            underline_color,
            link: link.cloned(),
        }
    }

    #[inline]
    pub(crate) fn sgr_word(&self) -> u64 {
        self.sgr
    }

    /// Return the foreground color.
    pub fn fg_color(&self) -> Option<Color> {
        crate::cell::storage::color_value(self.sgr)
    }
    /// Return the background color.
    pub fn bg_color(&self) -> Option<Color> {
        crate::cell::storage::color_value(self.sgr >> 25)
    }
    /// Return the explicit underline color.
    pub fn ul_color(&self) -> Option<Color> {
        self.underline_color
    }
    /// Return the attributes.
    pub fn attributes(&self) -> AttrFlags {
        AttrFlags::from_bits_truncate((self.sgr >> 50) as u16 & 0xff)
    }
    /// Return the underline shape.
    pub fn underline_shape(&self) -> UnderlineStyle {
        crate::cell::storage::underline_shape(self.sgr)
    }
    /// Borrow the hyperlink target.
    pub fn link_target(&self) -> Option<&Link> {
        self.link.as_deref()
    }
    pub(crate) fn link_arc(&self) -> Option<&Arc<Link>> {
        self.link.as_ref()
    }
    pub(crate) fn from_parts(parts: crate::cell::storage::StyleParts<'_>) -> Self {
        Self {
            sgr: crate::cell::storage::color_word(parts.fg)
                | (crate::cell::storage::color_word(parts.bg) << 25)
                | (u64::from(parts.attrs.bits() & 0xff) << 50)
                | ((parts.underline as u64) << 58),
            underline_color: parts.underline_color,
            link: parts.link.cloned(),
        }
    }
}

impl PartialEq for Style {
    fn eq(&self, other: &Self) -> bool {
        self.sgr == other.sgr
            && self.underline_color == other.underline_color
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
        self.sgr.hash(state);
        self.underline_color.hash(state);
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
    /// emits nothing, since the opener is additive; format any style with the
    /// alternate flag (`{style:#}`) to emit an explicit return to the terminal
    /// default.
    pub const EMPTY: Style = Style {
        sgr: 0,
        underline_color: None,
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
        self.sgr == 0 && self.underline_color.is_none()
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
        Self::from_parts(crate::cell::storage::StyleParts {
            fg: self.fg_color().or(base.fg_color()),
            bg: self.bg_color().or(base.bg_color()),
            underline_color: self.underline_color.or(base.underline_color),
            underline: if self.underline_shape() == UnderlineStyle::None {
                base.underline_shape()
            } else {
                self.underline_shape()
            },
            attrs: self.attributes() | base.attributes(),
            link: self.link.as_ref().or(base.link.as_ref()),
        })
    }

    /// Add bold intensity and return the updated style.
    ///
    /// This sets [`AttrFlags::BOLD`] and leaves all other fields unchanged.
    pub fn bold(mut self) -> Self {
        self.sgr |= u64::from(AttrFlags::BOLD.bits()) << 50;
        self
    }

    /// Add faint intensity and return the updated style.
    ///
    /// This sets [`AttrFlags::FAINT`] and leaves all other fields unchanged.
    pub fn faint(mut self) -> Self {
        self.sgr |= u64::from(AttrFlags::FAINT.bits()) << 50;
        self
    }

    /// Add italic text and return the updated style.
    ///
    /// This sets [`AttrFlags::ITALIC`] and leaves all other fields unchanged.
    pub fn italic(mut self) -> Self {
        self.sgr |= u64::from(AttrFlags::ITALIC.bits()) << 50;
        self
    }

    /// Use a single underline and return the updated style.
    ///
    /// Equivalent to [`Style::underline_style`] with
    /// [`UnderlineStyle::Single`].
    pub fn underline(mut self) -> Self {
        self.sgr = (self.sgr & !(7 << 58)) | ((UnderlineStyle::Single as u64) << 58);
        self
    }

    /// Add strikethrough text and return the updated style.
    ///
    /// This sets [`AttrFlags::STRIKETHROUGH`] and leaves all other fields
    /// unchanged.
    pub fn strikethrough(mut self) -> Self {
        self.sgr |= u64::from(AttrFlags::STRIKETHROUGH.bits()) << 50;
        self
    }

    /// Add slow blinking text and return the updated style.
    ///
    /// This sets [`AttrFlags::SLOW_BLINK`] and leaves all other fields
    /// unchanged.
    pub fn blink(mut self) -> Self {
        self.sgr |= u64::from(AttrFlags::SLOW_BLINK.bits()) << 50;
        self
    }

    /// Add rapid blinking text and return the updated style.
    ///
    /// This sets [`AttrFlags::RAPID_BLINK`] and leaves all other fields
    /// unchanged.
    pub fn rapid_blink(mut self) -> Self {
        self.sgr |= u64::from(AttrFlags::RAPID_BLINK.bits()) << 50;
        self
    }

    /// Reverse foreground and background and return the updated style.
    ///
    /// This sets [`AttrFlags::REVERSE`]; it does not swap the stored `fg` and
    /// `bg` values.
    pub fn reverse(mut self) -> Self {
        self.sgr |= u64::from(AttrFlags::REVERSE.bits()) << 50;
        self
    }

    /// Add concealed text and return the updated style.
    ///
    /// This sets [`AttrFlags::CONCEAL`] and leaves all other fields unchanged.
    pub fn conceal(mut self) -> Self {
        self.sgr |= u64::from(AttrFlags::CONCEAL.bits()) << 50;
        self
    }

    /// Set or clear the foreground color and return the updated style.
    ///
    /// Accepts any value convertible into `Option<Color>`, including a
    /// [`Color`] or `None`.
    /// Passing `None` clears any foreground color carried by the base style.
    pub fn fg(mut self, color: impl Into<Option<Color>>) -> Self {
        self.sgr = (self.sgr & !((1 << 25) - 1)) | crate::cell::storage::color_word(color.into());
        self
    }

    /// Set or clear the background color and return the updated style.
    ///
    /// Accepts any value convertible into `Option<Color>`, including a
    /// [`Color`] or `None`.
    /// Passing `None` clears any background color carried by the base style.
    pub fn bg(mut self, color: impl Into<Option<Color>>) -> Self {
        self.sgr = (self.sgr & !(((1 << 25) - 1) << 25))
            | (crate::cell::storage::color_word(color.into()) << 25);
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
        self.sgr = (self.sgr & !(7 << 58)) | ((style as u64) << 58);
        self
    }

    /// Replace the entire attribute flag set and return the updated style.
    ///
    /// This is useful when applying a previously computed [`AttrFlags`] value.
    /// Use the convenience builders when adding a single flag.
    pub fn attrs(mut self, attrs: AttrFlags) -> Self {
        self.sgr = (self.sgr & !(0xff << 50)) | (u64::from(attrs.bits() & 0xff) << 50);
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

    /// Write this style's opener bytes: the SGR sequence (`CSI … m`) followed
    /// by an OSC 8 hyperlink start when this style carries a [`link`](Self::link).
    ///
    /// Additive: an [empty](Self::is_empty) style writes nothing.
    fn write_opener<W: Write>(&self, w: &mut W) -> io::Result<()> {
        sgr::write_style(w, self)?;
        if let Some(link) = &self.link {
            crate::ansi::hyperlink::write_hyperlink(w, &link.url, &link.params)?;
        }
        Ok(())
    }

    /// Write this style's closer bytes: the OSC 8 hyperlink terminator when this
    /// style carries a [`link`](Self::link), followed by the SGR reset (`CSI m`)
    /// when it carries any SGR state.
    ///
    /// The SGR reset clears all attributes to their defaults, so the closer
    /// returns the terminal to a clean state rather than restoring whatever
    /// style was active before the opener. An [empty](Self::is_empty) style
    /// writes nothing.
    fn write_closer<W: Write>(&self, w: &mut W) -> io::Result<()> {
        if self.link.is_some() {
            w.write_all(crate::ansi::hyperlink::HYPERLINK_RESET)?;
        }
        if !self.is_sgr_empty() {
            w.write_all(sgr::RESET)?;
        }
        Ok(())
    }
}

/// Render this style as ANSI escape sequences.
///
/// The default form (`{style}`) renders the **opener**: the SGR sequence
/// (`CSI … m`) followed by an OSC 8 hyperlink start when this style carries a
/// [`link`](Style::link). The opener is additive, so an empty style renders
/// nothing.
///
/// The alternate form (`{style:#}`) renders the **closer**: the OSC 8
/// hyperlink terminator (when the style carries a link) followed by the SGR
/// reset (`CSI m`, when it carries SGR state). The SGR reset clears all
/// attributes to their defaults rather than restoring a previously active
/// style.
///
/// Use the two together to wrap a span with a single style value:
///
/// ```
/// use uncurses::color::Color;
/// use uncurses::style::Style;
///
/// let style = Style::default().bold().fg(Color::Green);
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
        assert!(s.attributes().contains(AttrFlags::BOLD));
        assert!(s.attributes().contains(AttrFlags::ITALIC));
        assert_eq!(s.fg_color(), Some(Color::Red));
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
    fn display_emits_sgr_opener_without_reset() {
        let s = Style::EMPTY.bold().fg(Color::Red);
        assert_eq!(format!("{s}"), "\x1b[1;31m");
    }

    #[test]
    fn display_empty_style_emits_nothing() {
        assert_eq!(format!("{}", Style::EMPTY), "");
    }

    #[test]
    fn span_wraps_text_in_style_and_reset() {
        let s = Style::EMPTY.bold();
        // SGR-only style: opener, text, then just the SGR reset (no link close).
        assert_eq!(format!("{s}hi{s:#}"), "\x1b[1mhi\x1b[m");
    }

    #[test]
    fn alternate_display_closes_only_what_is_set() {
        // A link adds the OSC 8 terminator before the SGR reset.
        let s = Style::EMPTY.bold().link("https://x", "");
        assert_eq!(format!("{s:#}"), "\x1b]8;;\x1b\\\x1b[m");

        // A link-only style emits just the OSC 8 terminator, no SGR reset.
        let s = Style::EMPTY.link("https://x", "");
        assert_eq!(format!("{s:#}"), "\x1b]8;;\x1b\\");

        // An empty style resets nothing.
        assert_eq!(format!("{:#}", Style::EMPTY), "");
    }

    #[test]
    fn inherit_self_wins_and_fills_unset_from_base() {
        let style = Style::EMPTY.italic().fg(Color::Blue);
        let base = Style::EMPTY.bold().fg(Color::Red).bg(Color::Black);
        let merged = style.inherit(base);
        // self's fg wins; bg is inherited from base; attributes combine.
        assert_eq!(merged.fg_color(), Some(crate::color::Color::Blue));
        assert_eq!(merged.bg_color(), Some(crate::color::Color::Black));
        assert!(merged.attributes().contains(AttrFlags::BOLD));
        assert!(merged.attributes().contains(AttrFlags::ITALIC));
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
    fn span_wraps_link_in_osc8() {
        let s = Style::EMPTY.underline().link("https://example.com", "");
        // SGR opener, hyperlink start, text, hyperlink end, SGR reset.
        assert_eq!(
            format!("{s}docs{s:#}"),
            "\x1b[4m\x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x1b\\\x1b[m"
        );
    }
}

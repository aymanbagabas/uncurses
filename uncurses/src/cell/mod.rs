//! Terminal cell values and grapheme segmentation.
//!
//! This module defines [`Cell`], the value stored by buffers and surfaces.
//! A cell combines grapheme content, visual style, and the number of
//! terminal columns the content occupies.
//!
//! ## Cell value type
//!
//! A [`Cell`] stores three pieces of state:
//!
//! - `content`: the grapheme cluster to display. Continuation cells have no
//!   content of their own.
//! - Visual style: colors, attributes, underline data, and a shared
//!   hyperlink. [`Cell::style`] returns these as an owned [`Style`].
//! - `width`: the cell's column footprint, returned by [`Cell::width`].
//!
//! Width also encodes the cell's structural role. A width of `1` or more
//! marks a primary cell that owns that many columns. A width of `0` marks a
//! continuation placeholder, whose column is owned by the primary to its
//! left.
//!
//! ## Construction
//!
//! Construct a cell with [`Cell::new`], passing its content, width, and style.
//! Measure the width with
//! [`WidthMode::grapheme_width`](crate::text::WidthMode::grapheme_width)
//! rather than assuming it. Use [`Style::EMPTY`] for the default style.
//! Use [`Cell::set_style`] to change the style of an existing cell.
//!
//! [`Cell::CONTINUATION`] is the placeholder that stands in the columns
//! after a multi-column grapheme. Most callers never write one; writing a
//! multi-column cell through
//! [`Buffer::set`](crate::buffer::Buffer::set) or
//! [`SurfaceMut::set_cell`](crate::buffer::SurfaceMut::set_cell) creates the
//! placeholders automatically.
//!
//! ```rust,ignore
//! use uncurses::cell::Cell;
//! use uncurses::style::Style;
//!
//! let a = Cell::new("a", 1, Style::EMPTY);
//! assert_eq!(a.width(), 1);
//!
//! let wide = Cell::new("中", 2, Style::EMPTY);
//! assert_eq!(wide.width(), 2);
//! ```
//!
//! ## Multi-column cells
//!
//! A grapheme wider than one column occupies that many adjacent grid
//! columns. The leftmost column stores the primary and every column after it
//! stores a continuation placeholder:
//!
//! ```text
//! col:    0       1       2
//!       ┌───────┬───────┬───┐
//! row 0 │ 中    │ cont. │ A │
//!       └───────┴───────┴───┘
//!         width=2 width=0 width=1
//! ```
//!
//! Most graphemes are one or two columns wide. A cell may be wider: under
//! [`WidthMode::Wc`](crate::text::WidthMode::Wc) a joined emoji sequence
//! measures the sum of its code points, so a four-person family emoji is
//! eight columns and owns seven continuations.
//!
//! Continuations preserve the one-`Cell`-per-column layout for wide
//! graphemes. [`Cell::is_blank`] identifies a cell that claims one column
//! and draws a space, regardless of style. A continuation claims zero
//! columns, so it returns `false`.

use std::sync::Arc;

use compact_str::CompactString;

mod placement;

use crate::color::Color;
use crate::style::{AttrFlags, Link, Style, UnderlineStyle};

/// A single terminal-grid cell.
///
/// `Cell` is the value stored in buffers and surfaces. It contains the
/// grapheme content for a column, the style applied to that content, and the
/// number of columns the content occupies.
///
/// Use [`Cell::new`] for construction and [`Cell::BLANK`] for an empty
/// styled-as-default space. Continuations are normally produced by the
/// surface write path rather than by application code.
#[derive(Debug)]
pub struct Cell {
    /// The grapheme cluster content. Empty string for a continuation
    /// placeholder.
    pub(crate) content: CompactString,
    pub(crate) fg: Option<Color>,
    pub(crate) bg: Option<Color>,
    pub(crate) underline_color: Option<Color>,
    pub(crate) attrs: AttrFlags,
    pub(crate) underline: UnderlineStyle,
    pub(crate) link: Option<Arc<Link>>,
    /// Column footprint. `0` marks a continuation placeholder; `1` or more
    /// marks a primary owning that many columns.
    pub(crate) width: u8,
}

impl PartialEq for Cell {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width && self.content == other.content && self.same_style(other)
    }
}

impl Clone for Cell {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            fg: self.fg,
            bg: self.bg,
            underline_color: self.underline_color,
            attrs: self.attrs,
            underline: self.underline,
            link: self.link.clone(),
            width: self.width,
        }
    }

    #[inline]
    fn clone_from(&mut self, source: &Self) {
        self.content.clone_from(&source.content);
        self.copy_style_from(source);
        self.width = source.width;
    }
}

impl Eq for Cell {}

impl Default for Cell {
    fn default() -> Self {
        Self::BLANK
    }
}

impl Cell {
    /// A blank one-column cell with default style.
    ///
    /// The blank cell stores a single space (`" "`), uses [`Style::EMPTY`],
    /// and has width `1`.
    ///
    /// # Returns
    ///
    /// This is a constant value, so use it directly wherever a default blank
    /// cell is needed.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Clearing and newly allocated buffers use `BLANK`. Clone it when an
    /// owned value is required.
    pub const BLANK: Cell = Cell {
        content: CompactString::const_new(" "),
        fg: None,
        bg: None,
        underline_color: None,
        attrs: AttrFlags::empty(),
        underline: UnderlineStyle::None,
        link: None,
        width: 1,
    };

    /// A continuation placeholder with default style.
    ///
    /// The placeholder stores no content, uses [`Style::EMPTY`], and has
    /// width `0`. [`Cell::is_continuation`] reports `true` for it.
    ///
    /// # Returns
    ///
    /// This is a constant value, so use it directly wherever a continuation
    /// is needed.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// A wide cell is followed by [`width`](Cell::width) `- 1` of these, one
    /// for every column the cell owns beyond its first. The surface write
    /// path lays them down; application code needs them only when it builds
    /// a row by hand.
    ///
    /// Under background color erase a continuation carries the style of the
    /// cell that owns it, so clone the constant and give it that style
    /// rather than leaving it default.
    pub const CONTINUATION: Cell = Cell {
        content: CompactString::const_new(""),
        fg: None,
        bg: None,
        underline_color: None,
        attrs: AttrFlags::empty(),
        underline: UnderlineStyle::None,
        link: None,
        width: 0,
    };

    /// Create a cell with `content`, `width` terminal columns, and `style`.
    ///
    /// # Parameters
    ///
    /// - `content`: grapheme content to store in the cell.
    /// - `width`: number of terminal columns the content occupies. Width `0`
    ///   makes a continuation placeholder whatever the content, so use
    ///   [`Cell::CONTINUATION`] when that is what you mean.
    /// - `style`: visual style to apply. An owned style transfers its
    ///   hyperlink. A borrowed style shares the hyperlink allocation.
    ///
    /// The `"\0p"` prefix at width one is reserved for private placement metadata.
    ///
    /// # Returns
    ///
    /// A cell with the given content, width, and style.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// This constructor does not validate that `width` matches the display
    /// width of `content`; the caller chooses the measurement policy. Use
    /// [`WidthMode::grapheme_width`](crate::text::WidthMode::grapheme_width)
    /// to measure, so the stored width matches what the terminal advances.
    ///
    /// When a cell wider than one column is written through the
    /// buffer/surface write path, the `width - 1` slots to its right are
    /// filled with continuation placeholders automatically. If there is no
    /// room for them at the row's right edge,
    /// [`Buffer::set`](crate::buffer::Buffer::set) stores a blank instead of
    /// part of a grapheme.
    #[inline]
    pub fn new(content: impl Into<CompactString>, width: u8, style: impl Into<Style>) -> Self {
        let mut cell = Cell {
            content: content.into(),
            width,
            ..Self::BLANK
        };
        cell.set_style(style);
        cell
    }

    /// Test whether this cell occupies more than one column.
    ///
    /// # Returns
    ///
    /// `true` when [`Cell::width`] is `2` or more.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Width alone decides the answer, because width alone decides how many
    /// columns the cell takes. A cell that stores no content still owns the
    /// columns its width names, and the blank drawn in its place covers all
    /// of them.
    ///
    /// In a well-formed surface, such a cell is followed immediately by
    /// `width - 1` continuation placeholders.
    #[inline]
    pub fn is_wide(&self) -> bool {
        self.width > 1
    }

    /// Test whether this is a continuation placeholder.
    ///
    /// # Returns
    ///
    /// `true` when the stored width is zero.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Ordinary width-zero content is skipped. Placement markers instead
    /// occupy one grid position and resolve through the screen's registry.
    #[inline]
    pub fn is_continuation(&self) -> bool {
        self.width == 0
    }

    /// Test whether this cell is a single blank column.
    ///
    /// # Returns
    ///
    /// `true` when the cell claims one column and draws a space in it, which
    /// is the shape of [`Cell::BLANK`].
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Style is not considered. A styled space is still blank, because this
    /// method answers what the cell occupies rather than how it is painted.
    ///
    /// Width is considered, so the answer stays about one column. A
    /// continuation claims none, and a cell claiming several stands for a
    /// span rather than a single blank; each is something other than a blank
    /// column, and each reports `false`.
    ///
    /// A cell that claims its one column while storing nothing draws a space
    /// there, the same as [`Cell::BLANK`], and answers the same way. The
    /// question is what reaches the column, so two cells that put the same
    /// thing in it give one answer.
    pub fn is_blank(&self) -> bool {
        self.width == 1 && (self.content == " " || self.content.is_empty())
    }

    /// The bytes that draw this cell.
    ///
    /// A cell storing no content still owns the columns its width names, so
    /// it draws as a blank in every one of them. Writing the content alone
    /// would close a gap the grid is holding open, and put everything after
    /// it on the row a column short for each one skipped.
    ///
    /// Call this only on a cell that is not a continuation; a continuation
    /// draws nothing at all, and its caller returns before reaching here.
    pub(crate) fn draw_bytes(&self) -> &[u8] {
        /// Enough spaces to stand in for any width a cell can claim.
        const BLANKS: [u8; u8::MAX as usize] = [b' '; u8::MAX as usize];

        if self.content.is_empty() {
            &BLANKS[..self.width as usize]
        } else {
            self.content.as_bytes()
        }
    }

    /// Return the cell's grapheme-cluster content.
    ///
    /// # Returns
    ///
    /// The stored content as `&str`. Continuation cells return an empty
    /// string.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// This returns the stored content exactly. For a placement cell this
    /// is metadata; use the screen to render its referenced content.
    #[inline]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    /// Column footprint of this cell on the grid.
    ///
    /// # Returns
    ///
    /// The number of terminal columns owned by this cell: `0` for a
    /// continuation, `1` for a placement fragment, otherwise the width passed
    /// to [`Cell::new`].
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Row walkers generally advance by `cell.width().max(1)` after handling
    /// continuations, so they make progress even when encountering a
    /// continuation placeholder.
    #[inline]
    pub fn width(&self) -> u8 {
        self.width
    }

    /// Return the visual style as an owned value.
    ///
    /// The returned style shares its hyperlink allocation with this cell.
    /// Changes to the returned style take effect after [`Self::set_style`].
    #[inline]
    pub fn style(&self) -> Style {
        Style {
            fg: self.fg,
            bg: self.bg,
            underline_color: self.underline_color,
            attrs: self.attrs,
            underline: self.underline,
            link: self.link.clone(),
        }
    }

    /// Replace the visual style without changing content or width.
    ///
    /// An owned style transfers its hyperlink. A borrowed style shares it.
    #[inline]
    pub fn set_style(&mut self, style: impl Into<Style>) {
        let style = style.into();
        self.fg = style.fg;
        self.bg = style.bg;
        self.underline_color = style.underline_color;
        self.attrs = style.attrs;
        self.underline = style.underline;
        self.link = style.link;
    }

    #[inline]
    pub(crate) fn copy_style_from(&mut self, source: &Self) {
        self.fg = source.fg;
        self.bg = source.bg;
        self.underline_color = source.underline_color;
        self.attrs = source.attrs;
        self.underline = source.underline;
        self.link.clone_from(&source.link);
    }

    #[inline]
    pub(crate) fn same_style(&self, other: &Self) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.underline_color == other.underline_color
            && self.attrs == other.attrs
            && self.underline == other.underline
            && self.link == other.link
    }

    #[inline]
    pub(crate) fn matches_style(&self, style: &Style) -> bool {
        self.fg == style.fg
            && self.bg == style.bg
            && self.underline_color == style.underline_color
            && self.attrs == style.attrs
            && self.underline == style.underline
            && self.link == style.link
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blank_cell() {
        let c = Cell::BLANK;
        assert_eq!(c.width(), 1);
        assert!(c.is_blank());
        assert!(c.style().is_empty());
    }

    #[test]
    fn test_narrow_cell() {
        let c = Cell::new("A", 1, Style::EMPTY);
        assert_eq!(c.content(), "A");
        assert!(!c.is_wide());
        assert_eq!(c.width(), 1);
    }

    #[test]
    fn test_wide_cell() {
        let c = Cell::new("中", 2, Style::EMPTY);
        assert_eq!(c.content(), "中");
        assert!(c.is_wide());
        assert_eq!(c.width(), 2);
    }

    #[test]
    fn test_continuation_cell() {
        let c = Cell::CONTINUATION;
        assert!(c.is_continuation());
        assert_eq!(c.width(), 0);
        assert!(!c.is_blank());
    }

    /// `is_blank` answers for one blank column, so width decides as much as
    /// content does.
    ///
    /// Reading it as "holds nothing to draw" put a continuation and a blank
    /// in the same class, though one claims no column and the other claims
    /// exactly one. A caller scanning a row for the columns it can drop then
    /// had to re-check the width every time to tell them apart.
    #[test]
    fn only_a_cell_drawing_one_blank_column_is_blank() {
        assert!(Cell::new(" ", 1, Style::EMPTY).is_blank());
        // Storing nothing in one column still draws a space there, which is
        // what `draw_bytes` puts on the screen.
        let empty = Cell::new("", 1, Style::EMPTY);
        assert_eq!(empty.draw_bytes(), b" ");
        assert!(empty.is_blank());
        // Style rides along; it says how the column is painted, not what it
        // holds.
        assert!(
            {
                let mut styled_cell = Cell::BLANK;
                styled_cell.set_style(Style::default().bold());
                styled_cell
            }
            .is_blank()
        );

        // A span of blank columns is not one blank column.
        assert!(!Cell::new(" ", 2, Style::EMPTY).is_blank());
        assert!(!Cell::new("", 2, Style::EMPTY).is_blank());
        assert!(!Cell::new("x", 1, Style::EMPTY).is_blank());
    }

    #[test]
    fn the_continuation_constant_matches_one_built_by_hand() {
        // Rows built before the constant existed pass `Cell::new("", 0, Style::EMPTY)`,
        // and `PartialEq` weighs content, width, and style. A constant that
        // differed in any of the three would compare unequal and make the
        // renderer redraw a column that did not change.
        assert_eq!(Cell::CONTINUATION, Cell::new("", 0, Style::EMPTY));
    }

    #[test]
    fn a_cell_may_own_more_than_two_columns() {
        // Under `WidthMode::Wc` a joined emoji sequence measures the sum of
        // its code points, so the grid must be able to credit one cell with
        // more columns than a CJK ideograph takes.
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let c = Cell::new(family, 8, Style::EMPTY);
        assert_eq!(c.width(), 8);
        assert!(c.is_wide());
        assert!(!c.is_continuation());
    }

    #[test]
    fn width_alone_decides_the_structural_role() {
        // Width `0`, `1`, and `2` or more partition every cell into a
        // continuation, a narrow cell, and a wide one. Content never enters
        // into it: a cell that claims no column has none to draw in, and one
        // that stores nothing still owns every column it claims.
        assert!(Cell::new("a", 0, Style::EMPTY).is_continuation());
        assert!(Cell::new("", 2, Style::EMPTY).is_wide());
        assert!(Cell::new("\u{1f469}", 2, Style::EMPTY).is_wide());
    }

    #[test]
    fn a_cell_claiming_no_column_is_a_continuation_whatever_it_stores() {
        // `grapheme_width` measures a lone combining mark as zero, and
        // `Cell::new`'s own advice is to measure with it, so a cell holding
        // content at width zero is one step from the documented path. It
        // claims no column, so the grid treats it as the placeholder it
        // structurally is and the surface declines to plant it loose.
        use crate::buffer::{Buffer, Surface, SurfaceMut};
        use crate::text::Encode;

        let mut buf = Buffer::new(3, 1);
        buf.set_cell((0, 0).into(), &Cell::new("A", 1, Style::EMPTY));
        buf.set_cell((1, 0).into(), &Cell::new("\u{301}", 0, Style::EMPTY));
        buf.set_cell((2, 0).into(), &Cell::new("B", 1, Style::EMPTY));

        assert!(Cell::new("\u{301}", 0, Style::EMPTY).is_continuation());
        assert_eq!(
            buf.cell((1, 0).into()).unwrap().content(),
            " ",
            "a loose continuation is declined, leaving the blank"
        );
        assert_eq!(buf.display().to_string(), "A B");
    }

    #[test]
    fn test_cell_with_style() {
        let c = Cell::new("x", 1, Style::default().bold());
        assert!(c.attrs.contains(crate::style::AttrFlags::BOLD));
    }

    #[test]
    fn style_roundtrip_preserves_all_fields() {
        for underline in [
            UnderlineStyle::None,
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Curly,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
        ] {
            let style = Style::new()
                .fg(Color::Rgb(12, 34, 56))
                .bg(Color::Indexed(200))
                .underline_color(Color::BrightCyan)
                .underline_style(underline)
                .attrs(AttrFlags::from_bits_retain(u16::MAX))
                .link("https://example.test/flat", "id=roundtrip");
            let cell = Cell::new("中", 2, &style);
            assert_eq!(cell.style(), style);
            assert_eq!(cell.fg, style.fg);
            assert_eq!(cell.bg, style.bg);
            assert_eq!(cell.underline_color, style.underline_color);
            assert_eq!(cell.underline, underline);
            assert_eq!(cell.attrs.bits(), u16::MAX);

            let mut changed = cell.style();
            changed.bg = None;
            let mut copy = cell.clone();
            copy.set_style(changed);
            assert_ne!(cell.style(), copy.style());
            assert_eq!(cell.style(), style);
            copy.set_style(Style::EMPTY);
            assert_eq!(copy.style(), Style::EMPTY);
            assert!(copy.link.as_deref().is_none());
            assert_eq!((copy.content(), copy.width()), ("中", 2));
        }

        #[cfg(target_pointer_width = "64")]
        assert_eq!(std::mem::size_of::<Cell>(), 48);
    }

    #[test]
    fn style_copies_and_long_text_clones_share_the_hyperlink() {
        use crate::buffer::{Buffer, Surface};

        let text = format!("中{}", "\u{301}".repeat(40));
        let style = Style::new()
            .fg(Color::Green)
            .bg(Color::Blue)
            .underline_color(Color::Red)
            .attrs(AttrFlags::from_bits_retain(u16::MAX))
            .underline_style(UnderlineStyle::Curly)
            .link("https://example.test/shared", "id=long");
        let shared = style.link.as_ref().unwrap();
        let mut source = Cell::new(text.as_str(), 2, &style);
        let mut continuation = Cell::CONTINUATION;
        continuation.copy_style_from(&source);
        assert_eq!(continuation.style(), source.style());
        assert!(continuation.is_continuation());
        assert_eq!(continuation.content(), "");
        assert!(std::ptr::eq(
            continuation.link.as_deref().unwrap(),
            shared.as_ref()
        ));
        continuation.copy_style_from(&Cell::BLANK);
        assert_eq!(continuation.style(), Style::EMPTY);
        assert_eq!((continuation.content(), continuation.width()), ("", 0));

        let mut copy = Cell::new("old", 1, Style::new().bold());
        copy.clone_from(&source);
        let mut buffer = Buffer::new(4, 1);
        buffer.set((0, 0), &source);
        source.set_style(Style::EMPTY);
        drop(source);
        assert_eq!(copy.content(), text);
        assert_eq!(copy.width(), 2);
        let extracted = copy.style();
        assert_eq!(extracted, style);
        assert!(Arc::ptr_eq(extracted.link.as_ref().unwrap(), shared));
        assert!(std::ptr::eq(copy.link.as_deref().unwrap(), shared.as_ref()));
        for x in 0..2 {
            let cell = buffer.cell((x, 0).into()).unwrap();
            assert_eq!(cell.style(), copy.style());
            assert!(std::ptr::eq(cell.link.as_deref().unwrap(), shared.as_ref()));
            assert_eq!(cell.is_continuation(), x == 1);
        }
        assert_eq!(buffer.cell((0, 0).into()).unwrap().content(), text);
    }

    #[test]
    fn style_comparisons_use_link_values_and_preserve_empty_links() {
        let a = Cell::new("A", 1, Style::new().link("https://example.test", "id=same"));
        let b = Cell::new(
            "中",
            2,
            Style::new().link("https://example.test", "id=same"),
        );
        assert!(!std::ptr::eq(
            a.link.as_deref().unwrap(),
            b.link.as_deref().unwrap()
        ));
        assert_eq!(a.style(), b.style());
        assert!(a.same_style(&b));
        assert!(a.matches_style(&b.style()));
        let mut same = b.clone();
        same.content.clone_from(&a.content);
        same.width = a.width;
        assert_eq!(a, same);
        assert_ne!(a, b);

        let empty_link = Style {
            link: Some(Arc::new(Link {
                url: String::new(),
                params: "id=empty".into(),
            })),
            ..Style::EMPTY
        };
        let mut cell = Cell::BLANK;
        cell.set_style(&empty_link);
        assert_ne!(cell, Cell::BLANK);
        assert_eq!(cell.style(), empty_link);
        assert!(cell.style().link.is_some());
        assert!(!cell.same_style(&Cell::BLANK));
        assert!(!cell.matches_style(&Style::EMPTY));

        for style in [
            Style::new().fg(Color::Green),
            Style::new().bg(Color::Blue),
            Style::new().underline_color(Color::Red),
            Style::new().bold(),
            Style::new().attrs(AttrFlags::from_bits_retain(1 << 15)),
            Style::new().underline_style(UnderlineStyle::Curly),
        ] {
            let changed = Cell::new(" ", 1, &style);
            assert!(!Cell::BLANK.same_style(&changed), "{style:?}");
            assert!(!Cell::BLANK.matches_style(&style), "{style:?}");
        }
    }
}

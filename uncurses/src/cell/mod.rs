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
//! - `style`: colors, attributes, underline data, and link metadata applied
//!   to the cell.
//! - `width`: the cell's column footprint, returned by [`Cell::width`].
//!
//! Width also encodes the cell's structural role. A width of `1` or more
//! marks a primary cell that owns that many columns. A width of `0` marks a
//! continuation placeholder, whose column is owned by the primary to its
//! left.
//!
//! ## Construction
//!
//! Construct every cell with [`Cell::new`], passing the content and the
//! number of columns it occupies. Measure that width with
//! [`WidthMode::grapheme_width`](crate::text::WidthMode::grapheme_width)
//! rather than assuming it. The constructor uses the default [`Style`]; use
//! [`Cell::style()`] to attach a style afterwards.
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
//!
//! let a = Cell::new("a", 1);
//! assert_eq!(a.width(), 1);
//!
//! let wide = Cell::new("中", 2);
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
//! Continuations are considered blank by [`Cell::is_blank`] because they do
//! not render independent content. They exist so row storage can preserve
//! the one-`Cell`-per-column layout while still representing wide graphemes
//! accurately.

use compact_str::CompactString;

use crate::style::Style;

/// A single terminal-grid cell.
///
/// `Cell` is the value stored in buffers and surfaces. It contains the
/// grapheme content for a column, the style applied to that content, and the
/// number of columns the content occupies.
///
/// Use [`Cell::new`] for construction and [`Cell::BLANK`] for an empty
/// styled-as-default space. Continuations are normally produced by the
/// surface write path rather than by application code.
#[derive(Debug, Clone)]
pub struct Cell {
    /// The grapheme cluster content. Empty string for a continuation
    /// placeholder.
    content: CompactString,
    /// Visual style: colors, attributes, underline, and any attached
    /// hyperlink. The link inside `style` is reference-counted so a
    /// run of identically-linked cells shares a single allocation
    /// without per-cell deep clones.
    pub style: Style,
    /// Column footprint. `0` marks a continuation placeholder; `1` or more
    /// marks a primary owning that many columns.
    width: u8,
}

impl PartialEq for Cell {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width && self.style == other.style && self.content == other.content
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
        style: Style::EMPTY,
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
        style: Style::EMPTY,
        width: 0,
    };

    /// Create a cell holding `content` across `width` terminal columns.
    ///
    /// # Parameters
    ///
    /// - `content`: grapheme content to store in the cell.
    /// - `width`: number of terminal columns the content occupies. Width `0`
    ///   makes a continuation placeholder whatever the content, so use
    ///   [`Cell::CONTINUATION`] when that is what you mean.
    ///
    /// # Returns
    ///
    /// A cell with the given content and width, and the default style.
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
    pub fn new(content: impl Into<CompactString>, width: u8) -> Self {
        Cell {
            content: content.into(),
            style: Style::default(),
            width,
        }
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
    /// `true` when [`Cell::width`] is `0`.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Width alone decides, as it does for [`Cell::is_wide`]: a cell that
    /// claims no column has none to draw in, so whatever it stores is never
    /// reached. Width `0`, `1`, and `2` or more partition every cell into a
    /// continuation, a narrow cell, and a wide one.
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
    /// This returns the stored content exactly; it does not derive or append
    /// the neighboring continuations for multi-column cells.
    #[inline]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    /// Column footprint of this cell on the grid.
    ///
    /// # Returns
    ///
    /// The number of terminal columns owned by this cell: `0` for a
    /// continuation placeholder, otherwise the width passed to
    /// [`Cell::new`].
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

    /// Return this cell with a replacement style.
    ///
    /// # Parameters
    ///
    /// - `style`: style to store on the returned cell.
    ///
    /// # Returns
    ///
    /// `self` with its `style` field replaced by `style`.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// This builder-style method preserves content and width. Styling a
    /// continuation is possible, but continuations do not render independent
    /// content.
    pub fn style(mut self, style: impl Into<Style>) -> Self {
        self.style = style.into();
        self
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
        assert!(c.style.is_empty());
    }

    #[test]
    fn test_narrow_cell() {
        let c = Cell::new("A", 1);
        assert_eq!(c.content(), "A");
        assert!(!c.is_wide());
        assert_eq!(c.width(), 1);
    }

    #[test]
    fn test_wide_cell() {
        let c = Cell::new("中", 2);
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
        assert!(Cell::BLANK.is_blank());
        assert!(Cell::new(" ", 1).is_blank());
        // Storing nothing in one column still draws a space there, which is
        // what `draw_bytes` puts on the screen.
        let empty = Cell::new("", 1);
        assert_eq!(empty.draw_bytes(), b" ");
        assert!(empty.is_blank());
        // Style rides along; it says how the column is painted, not what it
        // holds.
        assert!(Cell::BLANK.style(Style::default().bold()).is_blank());

        // A span of blank columns is not one blank column.
        assert!(!Cell::new(" ", 2).is_blank());
        assert!(!Cell::new("", 2).is_blank());
        // No column at all is not one blank column either.
        assert!(!Cell::CONTINUATION.is_blank());
        assert!(!Cell::new("x", 1).is_blank());
    }

    #[test]
    fn the_continuation_constant_matches_one_built_by_hand() {
        // Rows built before the constant existed pass `Cell::new("", 0)`,
        // and `PartialEq` weighs content, width, and style. A constant that
        // differed in any of the three would compare unequal and make the
        // renderer redraw a column that did not change.
        assert_eq!(Cell::CONTINUATION, Cell::new("", 0));
    }

    #[test]
    fn a_cell_may_own_more_than_two_columns() {
        // Under `WidthMode::Wc` a joined emoji sequence measures the sum of
        // its code points, so the grid must be able to credit one cell with
        // more columns than a CJK ideograph takes.
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let c = Cell::new(family, 8);
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
        assert!(Cell::CONTINUATION.is_continuation());
        assert!(Cell::new("a", 0).is_continuation());
        assert!(Cell::new("", 2).is_wide());
        assert!(Cell::new("\u{1f469}", 2).is_wide());
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
        buf.set_cell((0, 0).into(), &Cell::new("A", 1));
        buf.set_cell((1, 0).into(), &Cell::new("\u{301}", 0));
        buf.set_cell((2, 0).into(), &Cell::new("B", 1));

        assert!(Cell::new("\u{301}", 0).is_continuation());
        assert_eq!(
            buf.cell((1, 0).into()).unwrap().content(),
            " ",
            "a loose continuation is declined, leaving the blank"
        );
        assert_eq!(buf.display().to_string(), "A B");
    }

    #[test]
    fn test_cell_with_style() {
        let c = Cell::new("x", 1).style(Style::default().bold());
        assert!(c.style.attrs.contains(crate::style::AttrFlags::BOLD));
    }
}

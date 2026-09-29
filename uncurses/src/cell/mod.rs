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
//! `Cell::new("", 0)` creates the internal placeholder used for the columns
//! after a multi-column grapheme. Most callers should not write
//! continuations directly; writing a multi-column cell through
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

    /// Create a cell holding `content` across `width` terminal columns.
    ///
    /// # Parameters
    ///
    /// - `content`: grapheme content to store in the cell.
    /// - `width`: number of terminal columns the content occupies. Pass `0`
    ///   together with empty content to build a continuation placeholder.
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
    /// `true` when [`Cell::width`] is `2` or more and the content is not
    /// empty.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// In a well-formed surface, such a cell is followed immediately by
    /// `width - 1` continuation placeholders.
    #[inline]
    pub fn is_wide(&self) -> bool {
        self.width > 1 && !self.content.is_empty()
    }

    /// Test whether this is a continuation placeholder.
    ///
    /// # Returns
    ///
    /// `true` when [`Cell::width`] is `0` and the content is empty.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Continuations have width `0`, no content, and are considered blank.
    #[inline]
    pub fn is_continuation(&self) -> bool {
        self.width == 0 && self.content.is_empty()
    }

    /// Test whether this cell renders as blank space.
    ///
    /// # Returns
    ///
    /// `true` when the content is empty or the content is a single space.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Style is not considered. A styled space still counts as blank because
    /// this method answers whether the cell has independent textual content.
    /// Continuation placeholders are blank, because their content is empty.
    pub fn is_blank(&self) -> bool {
        self.content.is_empty() || self.content == " "
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
        let c = Cell::new("", 0);
        assert!(c.is_continuation());
        assert_eq!(c.width(), 0);
        assert!(c.is_blank());
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
    fn width_and_content_together_decide_the_structural_role() {
        // A continuation is empty and owns no column. A cell that is empty
        // but claims a column is malformed and is not a wide cell.
        assert!(Cell::new("", 0).is_continuation());
        assert!(!Cell::new("a", 0).is_continuation());
        assert!(!Cell::new("", 2).is_wide());
        assert!(Cell::new("\u{1f469}", 2).is_wide());
    }

    #[test]
    fn test_cell_with_style() {
        let c = Cell::new("x", 1).style(Style::default().bold());
        assert!(c.style.attrs.contains(crate::style::AttrFlags::BOLD));
    }
}

//! Terminal cell values and grapheme segmentation.
//!
//! This module defines [`Cell`], the value stored by buffers and surfaces.
//! A cell combines grapheme content, visual style, and a structural role
//! that describes whether it is a narrow cell, a wide-cell primary, or the
//! continuation column for a wide primary.
//!
//! ## Cell value type
//!
//! A [`Cell`] stores three pieces of state:
//!
//! - `content`: the grapheme cluster to display. Continuation cells have no
//!   content of their own.
//! - `style`: colors, attributes, underline data, and link metadata applied
//!   to the cell.
//! - [`Kind`]: the structural role that determines the cell's column
//!   footprint.
//!
//! The cell's display width is derived from its [`Kind`]: narrow cells
//! occupy one column, wide primaries occupy two columns, and continuation
//! placeholders report width `0` because their column is owned by the wide
//! primary to the left.
//!
//! ## Construction
//!
//! Construct cells with [`Cell::narrow`] for one-column graphemes and
//! [`Cell::wide`] for two-column graphemes. Both constructors use the
//! default [`Style`]. Use [`Cell::style()`] to attach a
//! style after construction.
//!
//! [`Cell::continuation`] creates the internal placeholder used for the
//! second column of a wide grapheme. Most callers should not write
//! continuations directly; writing a wide cell through
//! [`Buffer::set`](crate::buffer::Buffer::set) or
//! [`SurfaceMut::set_cell`](crate::buffer::SurfaceMut::set_cell) creates the
//! placeholder automatically.
//!
//! ```rust,ignore
//! use uncurses::cell::Cell;
//!
//! let a = Cell::narrow("a");
//! assert_eq!(a.width(), 1);
//!
//! let wide = Cell::wide("中");
//! assert_eq!(wide.width(), 2);
//! ```
//!
//! ## Wide cells
//!
//! A two-column grapheme occupies two adjacent grid columns. The left column
//! stores the wide primary and the right column stores a continuation
//! placeholder:
//!
//! ```text
//! col:    0       1       2
//!       ┌───────┬───────┬───┐
//! row 0 │ 中    │ cont. │ A │
//!       └───────┴───────┴───┘
//!         width=2 width=0 width=1
//! ```
//!
//! Continuations are considered blank by [`Cell::is_blank`] because they do
//! not render independent content. They exist so row storage can preserve
//! the one-`Cell`-per-column layout while still representing wide graphemes
//! accurately.

use compact_str::CompactString;

use crate::style::Style;

/// Structural role of a cell within a terminal grid.
///
/// `Kind` encodes the width relationship between adjacent cells. A wide
/// grapheme is stored as a [`Kind::Wide`] primary followed immediately by a
/// [`Kind::Continuation`] placeholder in the column to the right. Width is
/// derived from the kind by [`Cell::width`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A cell that occupies exactly one terminal column.
    Narrow,
    /// The primary cell of a two-column grapheme.
    ///
    /// The following column should contain [`Kind::Continuation`] when the
    /// cell is stored in a surface.
    Wide,
    /// The right-column placeholder for a [`Kind::Wide`] primary.
    ///
    /// A continuation carries no content of its own and reports width `0`.
    Continuation,
}

/// A single terminal-grid cell.
///
/// `Cell` is the value stored in buffers and surfaces. It contains the
/// grapheme content for a column, the style applied to that content, and a
/// [`Kind`] that determines whether the cell is a one-column value, a
/// two-column wide primary, or a continuation placeholder.
///
/// Use [`Cell::narrow`] and [`Cell::wide`] for normal construction. Use
/// [`Cell::BLANK`] for an empty styled-as-default space. Continuations are
/// normally produced by the surface write path rather than by application
/// code.
#[derive(Debug, Clone)]
pub struct Cell {
    /// The grapheme cluster content. Empty string for a wide-cell
    /// continuation placeholder.
    content: CompactString,
    /// Visual style: colors, attributes, underline, and any attached
    /// hyperlink. The link inside `style` is reference-counted so a
    /// run of identically-linked cells shares a single allocation
    /// without per-cell deep clones.
    pub style: Style,
    /// Structural kind: narrow, wide primary, or wide continuation.
    kind: Kind,
}

impl PartialEq for Cell {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.style == other.style && self.content == other.content
    }
}

impl Eq for Cell {}

impl Default for Cell {
    fn default() -> Self {
        Self::BLANK
    }
}

impl Cell {
    /// A blank narrow cell with default style.
    ///
    /// The blank cell stores a single space (`" "`), uses
    /// [`Style::EMPTY`], and has [`Kind::Narrow`].
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
        kind: Kind::Narrow,
    };

    /// Create a single-column cell with the given grapheme-cluster
    /// `content` and default style.
    ///
    /// # Parameters
    ///
    /// - `content`: grapheme content to store in the cell.
    ///
    /// # Returns
    ///
    /// A [`Kind::Narrow`] cell with width `1` and default style.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// This constructor does not validate display width. Call it only for
    /// content that should occupy one terminal column; use [`Cell::wide`] for
    /// two-column graphemes.
    pub fn narrow(content: impl Into<CompactString>) -> Self {
        Cell {
            content: content.into(),
            style: Style::default(),
            kind: Kind::Narrow,
        }
    }

    /// Create the primary cell of a two-column grapheme.
    ///
    /// # Parameters
    ///
    /// - `content`: grapheme content to store in the wide primary.
    ///
    /// # Returns
    ///
    /// A [`Kind::Wide`] cell with width `2` and default style.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// When a wide cell is written through the buffer/surface write path, the
    /// slot at `column + 1` is filled with a [`Kind::Continuation`]
    /// placeholder automatically. If there is no room for that placeholder
    /// at the row's right edge, [`Buffer::set`](crate::buffer::Buffer::set)
    /// stores a blank instead of half a wide grapheme.
    pub fn wide(content: impl Into<CompactString>) -> Self {
        Cell {
            content: content.into(),
            style: Style::default(),
            kind: Kind::Wide,
        }
    }

    /// Create a wide-character continuation placeholder.
    ///
    /// Continuation cells carry no content; they occupy the right
    /// half of a [`Cell::wide`] primary at the column to their left.
    ///
    /// # Returns
    ///
    /// A [`Kind::Continuation`] cell with empty content, default style, and
    /// width `0`.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Most callers should not construct continuations directly. Prefer
    /// writing a [`Cell::wide`] through a surface so the primary and
    /// continuation remain adjacent.
    pub fn continuation() -> Self {
        Cell {
            content: CompactString::default(),
            style: Style::default(),
            kind: Kind::Continuation,
        }
    }

    /// Return the cell's structural role.
    ///
    /// # Returns
    ///
    /// [`Kind::Narrow`], [`Kind::Wide`], or [`Kind::Continuation`].
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Use this when matching all roles explicitly. Predicate helpers such
    /// as [`Cell::is_wide`] are clearer for single-role checks.
    #[inline]
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Test whether this is a single-column cell.
    ///
    /// # Returns
    ///
    /// `true` when [`Cell::kind`] is [`Kind::Narrow`].
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Blank cells are narrow; continuation cells are not.
    #[inline]
    pub fn is_narrow(&self) -> bool {
        matches!(self.kind, Kind::Narrow)
    }

    /// Test whether this is the primary of a two-column grapheme.
    ///
    /// # Returns
    ///
    /// `true` when [`Cell::kind`] is [`Kind::Wide`].
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// In a well-formed surface, a wide cell is followed immediately by a
    /// continuation placeholder.
    #[inline]
    pub fn is_wide(&self) -> bool {
        matches!(self.kind, Kind::Wide)
    }

    /// Test whether this is a wide-character continuation placeholder.
    ///
    /// # Returns
    ///
    /// `true` when [`Cell::kind`] is [`Kind::Continuation`].
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
        matches!(self.kind, Kind::Continuation)
    }

    /// Test whether this cell renders as blank space.
    ///
    /// # Returns
    ///
    /// `true` when the content is empty, the content is a single space, or
    /// the cell is a continuation placeholder.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Style is not considered. A styled space still counts as blank because
    /// this method answers whether the cell has independent textual content.
    pub fn is_blank(&self) -> bool {
        self.content.is_empty() || self.content == " " || self.is_continuation()
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
    /// the neighboring continuation for wide cells.
    #[inline]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    /// Column footprint of this cell on the grid.
    ///
    /// - `Narrow` → 1
    /// - `Wide`   → 2
    /// - `Continuation` → 0 (the second slot of a wide primary)
    ///
    /// # Returns
    ///
    /// The number of terminal columns owned by this cell's role.
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
        match self.kind {
            Kind::Narrow => 1,
            Kind::Wide => 2,
            Kind::Continuation => 0,
        }
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
    /// This builder-style method preserves content and [`Kind`]. Styling a
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
        assert!(c.is_narrow());
        assert_eq!(c.width(), 1);
        assert!(c.is_blank());
        assert!(c.style.is_empty());
    }

    #[test]
    fn test_narrow_cell() {
        let c = Cell::narrow("A");
        assert_eq!(c.content(), "A");
        assert!(c.is_narrow());
        assert_eq!(c.width(), 1);
    }

    #[test]
    fn test_wide_cell() {
        let c = Cell::wide("中");
        assert_eq!(c.content(), "中");
        assert!(c.is_wide());
        assert_eq!(c.width(), 2);
    }

    #[test]
    fn test_continuation_cell() {
        let c = Cell::continuation();
        assert!(c.is_continuation());
        assert_eq!(c.width(), 0);
    }

    #[test]
    fn test_cell_with_style() {
        let c = Cell::narrow("x").style(Style::default().bold());
        assert!(c.style.attrs.contains(crate::style::AttrFlags::BOLD));
    }
}

//! Terminal cell values.
//!
//! Two types describe a grid position, one for reading and writing and one
//! for storage.
//!
//! [`Cell`] is the value applications work with. It owns its content and its
//! style outright, so it can be built, compared, and passed around without
//! reference to any particular grid.
//!
//! `Ref` is what a grid actually stores: three interned ids, eight bytes
//! total. Ids are issued by an [`arena`], and only that arena can turn them
//! back into content. Surfaces convert between the two at their boundary, so
//! id provenance never escapes the crate.
//!
//! ## What a cell holds
//!
//! - [`Content`]: the grapheme to display, either a single Unicode scalar or
//!   a multi-scalar cluster.
//! - [`Style`]: SGR appearance plus an optional hyperlink.
//! - [`Kind`]: the structural role that determines the column footprint.
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
//! Writing a wide cell through
//! [`SurfaceMut::set_cell`](crate::buffer::SurfaceMut::set_cell) lays down
//! the continuation automatically. Continuations report width `0` and count
//! as blank, because their column belongs to the primary on their left.
//!
//! ```rust
//! use uncurses::cell::{Cell, Content};
//! use uncurses::style::Style as Sgr;
//!
//! let a = Cell::new("a", Sgr::default().bold());
//! assert_eq!(a.width(), 1);
//! assert_eq!(a.content, Content::Char('a'));
//!
//! let wide = Cell::new("中", Sgr::default());
//! assert_eq!(wide.width(), 2);
//! ```

mod style;

pub use style::Style;

use std::fmt;

/// Structural role of a cell within a terminal grid.
///
/// `Kind` encodes the width relationship between adjacent cells. A wide
/// grapheme is stored as a [`Kind::Wide`] primary followed immediately by a
/// [`Kind::Continuation`] placeholder in the column to the right. Width is
/// derived from the kind by [`Cell::width`].
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Hash)]
// Discriminants are pinned because `Ref` packs them into the top bits of its
// content word. `u8` keeps the enum one byte; the cast to the wider id
// happens at the pack site.
#[repr(u8)]
pub enum Kind {
    /// A cell that occupies exactly one terminal column.
    #[default]
    Narrow = 0,
    /// The primary cell of a two-column grapheme.
    ///
    /// The following column should contain [`Kind::Continuation`] when the
    /// cell is stored in a surface.
    Wide = 1,
    /// The right-column placeholder for a [`Kind::Wide`] primary.
    ///
    /// A continuation carries no content of its own and reports width `0`.
    Continuation = 2,
}

impl Kind {
    /// Column footprint for this role: `1`, `2`, or `0` for a continuation.
    #[inline]
    pub fn width(self) -> u8 {
        match self {
            Kind::Narrow => 1,
            Kind::Wide => 2,
            Kind::Continuation => 0,
        }
    }

    /// The role a grapheme of `width` columns takes.
    #[inline]
    fn of_width(width: u8) -> Kind {
        if width >= 2 { Kind::Wide } else { Kind::Narrow }
    }
}

/// The grapheme a cell displays.
///
/// Nearly all terminal content is a single Unicode scalar, so that case is
/// stored inline and costs no allocation. Only a cluster of two or more
/// scalars -- an emoji sequence, a Devanagari conjunct, a base character
/// with combining marks -- needs owned storage.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Content {
    /// A single Unicode scalar.
    Char(char),
    /// A grapheme cluster of two or more scalars.
    Cluster(Box<str>),
}

impl Content {
    /// The single scalar this content holds, if it is one.
    ///
    /// # Returns
    ///
    /// `Some(char)` for [`Content::Char`], and `None` for a cluster or for
    /// empty content.
    ///
    /// # Panics
    ///
    /// Never panics.
    #[inline]
    pub fn char(&self) -> Option<char> {
        match self {
            Content::Char(c) => Some(*c),
            _ => None,
        }
    }

    /// Display width in terminal columns.
    ///
    /// # Parameters
    ///
    /// - `eaw_wide`: treat East Asian Ambiguous characters as two columns.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn width(&self, eaw_wide: bool) -> u8 {
        match self {
            Content::Char(c) => crate::text::char_width(*c, eaw_wide),
            Content::Cluster(s) => crate::text::grapheme_width(s, eaw_wide),
        }
    }
}

impl fmt::Display for Content {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Content::Char(c) => f.write_str(c.encode_utf8(&mut [0u8; 4])),
            Content::Cluster(s) => f.write_str(s),
        }
    }
}

impl From<&str> for Content {
    /// Classify text the way an [`Arena`] does: empty, one scalar, or a
    /// cluster.
    fn from(text: &str) -> Self {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            // Nothing to draw still owns a column, and it renders as a
            // space, so it is one. Keeping a separate "empty" spelling would
            // make two cells that look identical compare unequal.
            (None, _) => Content::Char(' '),
            (Some(c), None) => Content::Char(c),
            _ => Content::Cluster(text.into()),
        }
    }
}

impl From<char> for Content {
    fn from(c: char) -> Self {
        Content::Char(c)
    }
}

impl From<String> for Content {
    fn from(text: String) -> Self {
        Content::from(text.as_str())
    }
}

impl From<&String> for Content {
    fn from(text: &String) -> Self {
        Content::from(text.as_str())
    }
}

/// A single terminal-grid cell.
///
/// `Cell` owns everything it describes, so it can be built and compared
/// without an arena. Read one with
/// [`Surface::cell`](crate::buffer::Surface::cell) and write one with
/// [`SurfaceMut::set_cell`](crate::buffer::SurfaceMut::set_cell); the
/// surface interns and resolves at its own boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The grapheme to display.
    pub content: Content,
    /// SGR appearance and optional hyperlink.
    pub style: Style,
    /// Column footprint.
    pub kind: Kind,
}

impl Default for Cell {
    /// A blank narrow cell: one space in the default style.
    ///
    /// The blank is a space rather than empty content so that clearing a
    /// grid and writing spaces into it produce the same cell, which lets the
    /// renderer diff them away.
    fn default() -> Self {
        Cell {
            content: Content::Char(' '),
            style: Style::default(),
            kind: Kind::Narrow,
        }
    }
}

impl Cell {
    /// Create a cell displaying `content` in `style`.
    ///
    /// # Parameters
    ///
    /// - `content`: text or scalar to display. Text of two or more scalars
    ///   is kept as a cluster.
    /// - `style`: SGR appearance, or a [`Style`] carrying a hyperlink too.
    ///
    /// # Returns
    ///
    /// A cell whose [`Kind`] follows the content's display width: two-column
    /// graphemes become [`Kind::Wide`], everything else [`Kind::Narrow`].
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Width is measured with East Asian Ambiguous characters treated as
    /// narrow, so a cell built here may need its [`Cell::kind`] adjusted
    /// before being written to a surface running in the wide policy. The
    /// text-painting APIs measure against the surface and are the better
    /// choice when the content might be ambiguous.
    pub fn new(content: impl Into<Content>, style: impl Into<Style>) -> Self {
        let content = content.into();
        let kind = Kind::of_width(content.width(false));
        Cell {
            content,
            style: style.into(),
            kind,
        }
    }

    /// Create the right-column placeholder for a wide grapheme.
    ///
    /// # Returns
    ///
    /// A [`Kind::Continuation`] cell with no content and width `0`.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Writing a wide cell through a surface creates the continuation
    /// automatically, so this is rarely needed directly.
    pub fn continuation() -> Self {
        Cell {
            content: Content::Char(' '),
            style: Style::default(),
            kind: Kind::Continuation,
        }
    }

    /// Column footprint of this cell on the grid.
    ///
    /// # Returns
    ///
    /// `1` for narrow, `2` for wide, `0` for a continuation.
    ///
    /// # Panics
    ///
    /// Never panics.
    #[inline]
    pub fn width(&self) -> u8 {
        self.kind.width()
    }

    /// Test whether this is a single-column cell.
    #[inline]
    pub fn is_narrow(&self) -> bool {
        matches!(self.kind, Kind::Narrow)
    }

    /// Test whether this is the primary of a two-column grapheme.
    #[inline]
    pub fn is_wide(&self) -> bool {
        matches!(self.kind, Kind::Wide)
    }

    /// Test whether this is a wide-character continuation placeholder.
    #[inline]
    pub fn is_continuation(&self) -> bool {
        matches!(self.kind, Kind::Continuation)
    }

    /// Test whether this cell renders as blank space.
    ///
    /// # Returns
    ///
    /// `true` when the content is empty, is a single space, or the cell is a
    /// continuation placeholder. Style is not considered: a styled space is
    /// still blank, because this answers whether the cell has independent
    /// textual content.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn is_blank(&self) -> bool {
        match &self.content {
            Content::Char(' ') => true,
            _ => self.is_continuation(),
        }
    }
}

impl fmt::Display for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.content.fmt(f)
    }
}

impl<T: Into<Content>> From<T> for Cell {
    /// A cell displaying `content` in the default style.
    fn from(content: T) -> Self {
        Cell::new(content, Style::default())
    }
}

impl crate::buffer::GridCell for Cell {
    #[inline]
    fn width(&self) -> u8 {
        Cell::width(self)
    }
    #[inline]
    fn is_wide(&self) -> bool {
        Cell::is_wide(self)
    }
    #[inline]
    fn is_continuation(&self) -> bool {
        Cell::is_continuation(self)
    }
    #[inline]
    fn blank() -> Self {
        Cell::default()
    }
    #[inline]
    fn continuation() -> Self {
        Cell::continuation()
    }
    #[inline]
    fn blank_like(&self) -> Self {
        Cell {
            style: self.style.clone(),
            ..Cell::default()
        }
    }
    #[inline]
    fn continuation_like(&self) -> Self {
        Cell {
            style: self.style.clone(),
            ..Cell::continuation()
        }
    }
}

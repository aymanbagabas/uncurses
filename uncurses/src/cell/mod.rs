//! Terminal cell values and grapheme segmentation.
//!
//! This module defines portable [`Cell`] values and borrowed [`CellRef`] views.
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
//! Use [`Surface::new_cell`](crate::buffer::Surface::new_cell) to validate
//! a grapheme and measure it with a surface's width policy.
//! [`Cell::new`] accepts explicit widths for foreign grid imports.
//! [`Cell::style()`] attaches a style to an owned value.
//!
//! [`Cell::CONTINUATION`] is the placeholder that stands in the columns
//! after a multi-column grapheme. Most callers never write one; writing a
//! multi-column cell through
//! [`Buffer::set`](crate::buffer::Buffer::set) or
//! [`SurfaceMut::set_cell`](crate::buffer::SurfaceMut::set_cell) creates the
//! placeholders automatically.
//!
//! ```rust
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
//! Continuations preserve the one-`Cell`-per-column layout for wide
//! graphemes. [`Cell::is_blank`] identifies a cell that claims one column
//! and draws a space, regardless of style. A continuation claims zero
//! columns, so it returns `false`.

use compact_str::CompactString;

use crate::style::Style;

pub(crate) mod storage;

use crate::color::Color;
use crate::style::{AttrFlags, Link, UnderlineStyle};
use storage::{Glyph, PackedCell, Resources, StyleParts};

/// A borrowed cell value with direct content, width, and style accessors.
///
/// Surface reads borrow the source resources. Copying this view preserves
/// the owner's resource counts. The view stays valid for its source borrow.
/// Use [`Self::to_owned`] for a portable snapshot, or [`Self::to_style`]
/// for an owned style. [`Self::link`] borrows the shared hyperlink target.
#[derive(Clone, Copy)]
pub struct CellRef<'a> {
    source: RefSource<'a>,
    scalar: [u8; 4],
    scalar_len: u8,
}

#[derive(Clone, Copy)]
enum RefSource<'a> {
    Packed(&'a PackedCell, &'a Resources),
    Text(TextRef<'a>),
}

#[derive(Clone, Copy)]
enum TextRef<'a> {
    Owned(&'a Cell),
    Borrowed(&'a BorrowedCell<'a>),
}

impl<'a> TextRef<'a> {
    #[inline]
    fn content(self) -> &'a str {
        match self {
            Self::Owned(cell) => cell.content(),
            Self::Borrowed(cell) => cell.content,
        }
    }

    #[inline]
    fn width(self) -> u8 {
        match self {
            Self::Owned(cell) => cell.width(),
            Self::Borrowed(cell) => cell.width,
        }
    }

    #[inline]
    fn style(self) -> &'a Style {
        match self {
            Self::Owned(cell) => &cell.style,
            Self::Borrowed(cell) => cell.style,
        }
    }
}

/// A transient text cell. Storage imports its borrowed content and style.
pub(crate) struct BorrowedCell<'a> {
    content: &'a str,
    width: u8,
    style: &'a Style,
}

impl<'a> BorrowedCell<'a> {
    pub(crate) fn new(content: &'a str, width: u8, style: &'a Style) -> Self {
        Self {
            content,
            width,
            style,
        }
    }

    pub(crate) fn as_ref(&self) -> CellRef<'_> {
        CellRef {
            source: RefSource::Text(TextRef::Borrowed(self)),
            scalar: [0; 4],
            scalar_len: 0,
        }
    }
}

impl<'a> CellRef<'a> {
    fn text_import_source(&self) -> Option<(&'a str, u8, &'a Style)> {
        match self.source {
            RefSource::Text(cell) => Some((cell.content(), cell.width(), cell.style())),
            _ => None,
        }
    }

    fn packed_import_source(&self) -> Option<(PackedCell, &'a Resources)> {
        match self.source {
            RefSource::Packed(cell, resources) => Some((*cell, resources)),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn packed(cell: &'a PackedCell, resources: &'a Resources) -> Self {
        let mut scalar = [0; 4];
        let scalar_len = cell
            .scalar()
            .map_or(0, |c| c.encode_utf8(&mut scalar).len() as u8);
        Self {
            source: RefSource::Packed(cell, resources),
            scalar,
            scalar_len,
        }
    }

    /// Return the scalar when the content contains exactly one scalar.
    #[inline]
    pub fn scalar(&self) -> Option<char> {
        match self.source {
            RefSource::Packed(cell, _) => cell.scalar(),
            _ => {
                let mut chars = self.content().chars();
                let first = chars.next()?;
                chars.next().is_none().then_some(first)
            }
        }
    }
    /// Return complex content, or `None` for a scalar or empty cell.
    #[inline]
    pub fn grapheme(&self) -> Option<&str> {
        match self.source {
            RefSource::Packed(cell, resources) => match resources.glyph(*cell) {
                Glyph::Str(s) => Some(s),
                _ => None,
            },
            _ => {
                let s = self.content();
                s.chars().nth(1).is_some().then_some(s)
            }
        }
    }
    /// Return the cell content.
    #[inline]
    pub fn content(&self) -> &str {
        match self.source {
            RefSource::Packed(_, _) if self.scalar_len != 0 => {
                std::str::from_utf8(&self.scalar[..self.scalar_len as usize]).unwrap()
            }
            RefSource::Packed(cell, resources) => match resources.glyph(*cell) {
                Glyph::Empty => "",
                Glyph::Str(s) => s,
                Glyph::Scalar(_) => unreachable!("scalar content is inline"),
            },
            RefSource::Text(cell) => cell.content(),
        }
    }
    #[inline]
    pub(crate) fn content_bytes(&self) -> &[u8] {
        if self.scalar_len != 0 {
            &self.scalar[..self.scalar_len as usize]
        } else {
            self.content().as_bytes()
        }
    }
    /// Return the stored column width.
    #[inline]
    pub fn width(&self) -> u8 {
        match self.source {
            RefSource::Packed(cell, _) => cell.width(),
            RefSource::Text(cell) => cell.width(),
        }
    }
    /// Test for a one-column space.
    #[inline]
    pub fn is_blank(&self) -> bool {
        match self.source {
            RefSource::Packed(cell, _) => cell.is_blank(),
            RefSource::Text(cell) => {
                cell.width() == 1 && (cell.content().is_empty() || cell.content() == " ")
            }
        }
    }
    #[inline]
    pub(crate) fn draw_bytes(&self) -> &[u8] {
        const BLANKS: [u8; u8::MAX as usize] = [b' '; u8::MAX as usize];
        if self.is_continuation() {
            b""
        } else {
            let bytes = self.content_bytes();
            if bytes.is_empty() {
                &BLANKS[..self.width() as usize]
            } else {
                bytes
            }
        }
    }
    /// Test for a continuation column.
    #[inline]
    pub fn is_continuation(&self) -> bool {
        self.width() == 0
    }
    /// Test for a primary that spans multiple columns.
    #[inline]
    pub fn is_wide(&self) -> bool {
        self.width() > 1
    }
    /// Return the foreground color.
    #[inline]
    pub fn fg_color(&self) -> Option<Color> {
        match self.source {
            RefSource::Packed(cell, _) => storage::color_value(cell.sgr_word()),
            RefSource::Text(cell) => cell.style().fg_color(),
        }
    }
    /// Return the background color.
    #[inline]
    pub fn bg_color(&self) -> Option<Color> {
        match self.source {
            RefSource::Packed(cell, _) => storage::color_value(cell.sgr_word() >> 25),
            RefSource::Text(cell) => cell.style().bg_color(),
        }
    }
    /// Return the explicit underline color.
    #[inline]
    pub fn ul_color(&self) -> Option<Color> {
        match self.source {
            RefSource::Packed(cell, resources) => resources.underline_color(*cell),
            RefSource::Text(cell) => cell.style().ul_color(),
        }
    }
    /// Return the attributes.
    #[inline]
    pub fn attributes(&self) -> AttrFlags {
        match self.source {
            RefSource::Packed(cell, _) => {
                AttrFlags::from_bits_truncate(((cell.sgr_word() >> 50) & 0xff) as u16)
            }
            RefSource::Text(cell) => cell.style().attributes(),
        }
    }
    /// Return the underline shape.
    #[inline]
    pub fn underline_shape(&self) -> UnderlineStyle {
        match self.source {
            RefSource::Packed(cell, _) => storage::underline_shape(cell.sgr_word()),
            RefSource::Text(cell) => cell.style().underline_shape(),
        }
    }
    /// Borrow the hyperlink target.
    #[inline]
    pub fn link(&self) -> Option<&Link> {
        match self.source {
            RefSource::Packed(cell, resources) => resources
                .extras(*cell)
                .and_then(|extras| extras.link.as_deref()),
            RefSource::Text(cell) => cell.style().link_target(),
        }
    }
    /// Make an owned style with the same components.
    ///
    /// The hyperlink retains its original shared [`std::sync::Arc`] allocation.
    pub fn to_style(&self) -> Style {
        if let RefSource::Packed(cell, resources) = self.source {
            return Style::from_packed_sgr(
                cell.sgr_word(),
                resources.underline_color(*cell),
                resources
                    .extras(*cell)
                    .and_then(|extras| extras.link.as_ref()),
            );
        }
        self.parts().to_owned()
    }
    /// Make a portable snapshot of the content, stored width, and complete style.
    ///
    /// The snapshot can outlive the source. Its hyperlink retains the original
    /// shared [`std::sync::Arc`] allocation.
    pub fn to_owned(&self) -> Cell {
        Cell::new(self.content(), self.width()).style(self.to_style())
    }
    #[inline]
    pub(crate) fn style_eq(&self, style: &Style) -> bool {
        match self.source {
            RefSource::Packed(cell, resources) => resources.style_eq(*cell, style),
            RefSource::Text(cell) => cell.style() == style,
        }
    }
    #[inline]
    pub(crate) fn style_eq_ref(&self, other: Self) -> bool {
        match (self.source, other.source) {
            (RefSource::Packed(cell, resources), RefSource::Packed(other, owner)) => {
                resources.styles_eq(*cell, owner, *other)
            }
            (_, RefSource::Text(cell)) => self.style_eq(cell.style()),
            (RefSource::Text(cell), _) => other.style_eq(cell.style()),
        }
    }
    #[inline]
    pub(crate) fn parts(&self) -> StyleParts<'a> {
        match self.source {
            RefSource::Packed(cell, resources) => resources.style(*cell),
            RefSource::Text(cell) => StyleParts::from_style(cell.style()),
        }
    }
    #[inline]
    pub(crate) fn glyph(&self) -> Glyph<'_> {
        match self.source {
            RefSource::Packed(cell, resources) => resources.glyph(*cell),
            RefSource::Text(cell) => Glyph::Str(cell.content()),
        }
    }
}

impl<'a> From<&'a Cell> for CellRef<'a> {
    #[inline]
    fn from(cell: &'a Cell) -> Self {
        Self {
            source: RefSource::Text(TextRef::Owned(cell)),
            scalar: [0; 4],
            scalar_len: 0,
        }
    }
}
impl PartialEq for CellRef<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        if let (RefSource::Packed(a, owner), RefSource::Packed(b, other_owner)) =
            (self.source, other.source)
        {
            return owner.value_eq(*a, other_owner, *b);
        }
        self.text_eq(other)
    }
}

impl CellRef<'_> {
    // Keep text decoding out of the packed comparison path.
    #[inline(never)]
    fn text_eq(&self, other: &Self) -> bool {
        match (self.source, other.source) {
            (RefSource::Text(a), RefSource::Text(b)) => {
                a.width() == b.width() && a.style() == b.style() && a.content() == b.content()
            }
            (_, RefSource::Text(cell)) => {
                cell.width() == self.width()
                    && cell.content().as_bytes() == self.content_bytes()
                    && self.style_eq(cell.style())
            }
            (RefSource::Text(cell), _) => {
                cell.width() == other.width()
                    && cell.content().as_bytes() == other.content_bytes()
                    && other.style_eq(cell.style())
            }
            _ => unreachable!("packed cells use packed equality"),
        }
    }
}
impl std::fmt::Debug for CellRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CellRef")
            .field("content", &self.content())
            .field("width", &self.width())
            .field("style", &self.parts())
            .finish()
    }
}
impl Eq for CellRef<'_> {}
impl PartialEq<Cell> for CellRef<'_> {
    fn eq(&self, other: &Cell) -> bool {
        *self == other.as_ref()
    }
}
impl PartialEq<&Cell> for CellRef<'_> {
    fn eq(&self, other: &&Cell) -> bool {
        *self == other.as_ref()
    }
}
impl PartialEq<CellRef<'_>> for Cell {
    fn eq(&self, other: &CellRef<'_>) -> bool {
        self.as_ref() == *other
    }
}

/// A cell factory error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellError {
    /// The input contains no grapheme.
    Empty,
    /// The input contains multiple graphemes.
    MultipleGraphemes,
    /// The grapheme occupies zero columns.
    ZeroWidth,
}
impl std::fmt::Display for CellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Empty => "cell content is empty",
            Self::MultipleGraphemes => "cell content must contain exactly one grapheme",
            Self::ZeroWidth => "cell content must occupy at least one column",
        })
    }
}
impl std::error::Error for CellError {}

/// A single terminal-grid cell.
///
/// `Cell` is a portable value for imports, snapshots, and reusable content.
/// It contains the content, style, and explicit column width.
/// Buffer reads return [`CellRef`] values that borrow compact storage.
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
    /// Borrow this portable cell without a resource allocation.
    pub fn as_ref(&self) -> CellRef<'_> {
        self.into()
    }
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
    /// This constructor retains the supplied symbol and explicit width.
    /// It supports foreign grids whose symbols contain arbitrary text,
    /// including several graphemes. Structural copies preserve that width.
    /// Use [`Surface::new_cell`](crate::buffer::Surface::new_cell) to validate
    /// one grapheme and measure it with a surface's width policy instead.
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
    #[cfg(test)]
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
    fn packed_style_snapshot_preserves_explicit_underline_and_shared_link() {
        use std::sync::Arc;

        let colors = [
            None,
            Some(Color::Red),
            Some(Color::Indexed(1)),
            Some(Color::Rgb(3, 5, 7)),
        ];
        let linked = Style::EMPTY.link("https://example.com/snapshot", "id=snapshot");
        let mut resources = Resources::default();
        for fg in colors {
            for bg in colors {
                for ul in colors {
                    let style = linked
                        .clone()
                        .fg(fg)
                        .bg(bg)
                        .underline_color(ul)
                        .underline_style(UnderlineStyle::Curly)
                        .attrs(AttrFlags::all());
                    let packed = resources.intern(&Cell::BLANK.style(&style));
                    let snapshot = CellRef::packed(&packed, &resources).to_style();
                    assert_eq!(snapshot, style);
                    assert!(Arc::ptr_eq(
                        snapshot.link_arc().unwrap(),
                        linked.link_arc().unwrap()
                    ));
                    let changed = snapshot.fg(Color::Blue).bg(Color::Green);
                    assert_eq!(changed.ul_color(), ul);
                    resources.release(packed);
                }
            }
        }
    }

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
        use crate::buffer::{Buffer, Surface};
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
        assert!(c.style.attributes().contains(crate::style::AttrFlags::BOLD));
    }
}

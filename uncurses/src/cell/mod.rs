pub mod segment;

pub use segment::graphemes;

use compact_str::CompactString;

use crate::style::Style;

/// Structural kind of a cell within the grid.
///
/// Encodes wide-cell invariants in the type system: a wide grapheme
/// occupies a [`Kind::Wide`] primary cell followed immediately by
/// a [`Kind::Continuation`] placeholder in the column to the
/// right. The previous magic-number `width: u8` field is replaced by
/// this enum so callers don't have to translate between `0`, `1`, and
/// `2` to reason about cell roles.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Single-column glyph cell.
    Narrow,
    /// Primary cell of a two-column grapheme. The cell at `column + 1`
    /// carries [`Kind::Continuation`].
    Wide,
    /// Right-half placeholder of a [`Kind::Wide`] primary at the
    /// column to the left. Carries no content of its own.
    Continuation,
}

/// A single terminal cell.
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
    pub const BLANK: Cell = Cell {
        content: CompactString::const_new(" "),
        style: Style::EMPTY,
        kind: Kind::Narrow,
    };

    /// Create a single-column cell with the given grapheme-cluster
    /// `content` and default style.
    pub fn narrow(content: impl Into<CompactString>) -> Self {
        Cell {
            content: content.into(),
            style: Style::default(),
            kind: Kind::Narrow,
        }
    }

    /// Create the primary cell of a two-column grapheme.
    ///
    /// When a wide cell is written into a buffer the slot at
    /// `column + 1` is filled with a [`Kind::Continuation`]
    /// placeholder automatically by the buffer layer.
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
    pub fn continuation() -> Self {
        Cell {
            content: CompactString::default(),
            style: Style::default(),
            kind: Kind::Continuation,
        }
    }

    /// The cell's structural kind.
    #[inline]
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Whether this cell is a single-column glyph cell.
    #[inline]
    pub fn is_narrow(&self) -> bool {
        matches!(self.kind, Kind::Narrow)
    }

    /// Whether this cell is the primary of a two-column grapheme.
    #[inline]
    pub fn is_wide(&self) -> bool {
        matches!(self.kind, Kind::Wide)
    }

    /// Whether this cell is a wide-character continuation placeholder.
    #[inline]
    pub fn is_continuation(&self) -> bool {
        matches!(self.kind, Kind::Continuation)
    }

    /// Whether this cell is a blank/space.
    pub fn is_blank(&self) -> bool {
        self.content.is_empty() || self.content == " " || self.is_continuation()
    }

    /// The cell's grapheme-cluster content. Empty for a wide-cell
    /// continuation placeholder.
    #[inline]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    /// Column footprint of this cell on the grid.
    ///
    /// - `Narrow` → 1
    /// - `Wide`   → 2
    /// - `Continuation` → 0 (the second slot of a wide primary)
    #[inline]
    pub fn width(&self) -> u8 {
        match self.kind {
            Kind::Narrow => 1,
            Kind::Wide => 2,
            Kind::Continuation => 0,
        }
    }

    /// Return a copy of this cell with the given style.
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
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

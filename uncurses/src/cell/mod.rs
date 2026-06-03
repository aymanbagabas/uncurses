pub mod segment;

pub use segment::graphemes;

use compact_str::CompactString;

use crate::style::Style;

/// Shape of a cell within the grid.
///
/// Replaces the old `width: u8` field with a typed enum so wide-cell
/// invariants and (in the future) richer multi-cell content can be
/// expressed structurally rather than as magic numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellKind {
    /// Single-column cell.
    Narrow,
    /// Primary cell of a two-column grapheme. The body cell at
    /// `column + 1` carries [`CellKind::Continuation`].
    Wide,
    /// Body cell of a [`CellKind::Wide`] primary at the column to the
    /// left. Carries no content of its own.
    Continuation,
}

impl CellKind {
    /// Display-width contribution of a cell with this kind:
    /// 1 for [`Narrow`], 2 for [`Wide`], 0 for [`Continuation`].
    #[inline]
    pub fn width(&self) -> u8 {
        match self {
            CellKind::Narrow => 1,
            CellKind::Wide => 2,
            CellKind::Continuation => 0,
        }
    }
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
    style: Style,
    /// Structural shape of this cell. Drives display-width arithmetic
    /// and the renderer's wide-cell handling.
    kind: CellKind,
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
    /// A blank cell with default style and width 1.
    pub const BLANK: Cell = Cell {
        content: CompactString::const_new(" "),
        style: Style::EMPTY,
        kind: CellKind::Narrow,
    };

    /// Create a single-column (narrow) cell with the given grapheme
    /// `content` and the empty style.
    pub fn narrow(content: impl Into<CompactString>) -> Self {
        Cell {
            content: content.into(),
            style: Style::EMPTY,
            kind: CellKind::Narrow,
        }
    }

    /// Create a two-column (wide) primary cell with the given grapheme
    /// `content` and the empty style. The continuation cell at
    /// `column + 1` must be a [`Cell::continuation`].
    pub fn wide(content: impl Into<CompactString>) -> Self {
        Cell {
            content: content.into(),
            style: Style::EMPTY,
            kind: CellKind::Wide,
        }
    }

    /// Create a continuation cell — the second column of a wide
    /// grapheme. Carries no content of its own.
    pub fn continuation() -> Self {
        Cell {
            content: CompactString::const_new(""),
            style: Style::EMPTY,
            kind: CellKind::Continuation,
        }
    }

    /// Whether this cell is a blank/space.
    pub fn is_blank(&self) -> bool {
        self.content.is_empty()
            || self.content == " "
            || matches!(self.kind, CellKind::Continuation)
    }

    /// Whether this is a wide-char continuation cell.
    #[inline]
    pub fn is_continuation(&self) -> bool {
        matches!(self.kind, CellKind::Continuation)
    }

    /// The cell's grapheme-cluster content. Empty for a wide-cell
    /// continuation placeholder.
    #[inline]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    /// The cell's display width: 1 for narrow, 2 for wide, 0 for the
    /// continuation placeholder that follows a wide cell.
    #[inline]
    pub fn width(&self) -> u8 {
        self.kind.width()
    }

    /// The cell's structural kind.
    #[inline]
    pub fn kind(&self) -> &CellKind {
        &self.kind
    }

    /// The cell's style (colors, attributes, underline, and any
    /// attached hyperlink).
    #[inline]
    pub fn style(&self) -> &Style {
        &self.style
    }

    /// Return a copy of this cell with the given style.
    pub fn with_style(mut self, style: Style) -> Self {
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
        assert_eq!(c.width(), 1);
        assert!(c.is_blank());
        assert!(c.style().is_empty());
    }

    #[test]
    fn test_narrow_cell() {
        let c = Cell::narrow("A");
        assert_eq!(c.content(), "A");
        assert_eq!(c.width(), 1);
        assert_eq!(c.kind(), &CellKind::Narrow);
    }

    #[test]
    fn test_wide_cell() {
        let c = Cell::wide("あ");
        assert_eq!(c.content(), "あ");
        assert_eq!(c.width(), 2);
        assert_eq!(c.kind(), &CellKind::Wide);
    }

    #[test]
    fn test_continuation_cell() {
        let c = Cell::continuation();
        assert_eq!(c.width(), 0);
        assert!(c.is_continuation());
        assert_eq!(c.kind(), &CellKind::Continuation);
    }

    #[test]
    fn test_cell_with_style() {
        let c = Cell::narrow("x").with_style(Style::EMPTY.with_bold());
        assert!(c.style().attrs.contains(crate::style::AttrFlags::BOLD));
    }
}

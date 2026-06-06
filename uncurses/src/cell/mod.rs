pub mod segment;

pub use segment::graphemes;

use compact_str::CompactString;

use crate::layout::Rect;
use crate::style::Style;

/// Structural kind of a cell within the grid.
///
/// Replaces the old `width: u8` field with a typed enum so wide-cell
/// invariants and richer multi-cell content can be expressed
/// structurally rather than as magic numbers.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum CellKind {
    /// Single-column glyph cell.
    Narrow,
    /// Primary cell of a two-column grapheme. The cell at `column + 1`
    /// carries [`CellKind::Continuation`].
    Wide,
    /// Right-half placeholder of a [`CellKind::Wide`] primary at the
    /// column to the left. Carries no content of its own.
    Continuation,
    /// A cell that belongs to a rich-content rectangle — addon-managed
    /// content the renderer must treat as opaque (e.g. a raster image
    /// painted via a control sequence). The cell at `(rect.x, rect.y)`
    /// is the anchor and carries the payload bytes in [`Cell::content`];
    /// cells elsewhere in `rect` are body placeholders with empty
    /// content.
    Rect(Rect),
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
    /// Structural kind: narrow, wide primary, wide continuation, or
    /// rect cell.
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
    /// A blank narrow cell with default style.
    pub const BLANK: Cell = Cell {
        content: CompactString::const_new(" "),
        style: Style::EMPTY,
        kind: CellKind::Narrow,
    };

    /// Create a single-column cell with the given grapheme-cluster
    /// `content` and default style.
    pub fn narrow(content: impl Into<CompactString>) -> Self {
        Cell {
            content: content.into(),
            style: Style::EMPTY,
            kind: CellKind::Narrow,
        }
    }

    /// Create the primary cell of a two-column grapheme.
    ///
    /// When a wide cell is written into a buffer the slot at
    /// `column + 1` is filled with a [`CellKind::Continuation`]
    /// placeholder automatically by the buffer layer.
    pub fn wide(content: impl Into<CompactString>) -> Self {
        Cell {
            content: content.into(),
            style: Style::EMPTY,
            kind: CellKind::Wide,
        }
    }

    /// Create a wide-character continuation placeholder.
    ///
    /// Continuation cells carry no content; they occupy the right
    /// half of a [`Cell::wide`] primary at the column to their left.
    pub fn continuation() -> Self {
        Cell {
            content: CompactString::default(),
            style: Style::EMPTY,
            kind: CellKind::Continuation,
        }
    }

    /// Create a rect anchor cell carrying the payload bytes the
    /// renderer will emit verbatim at `(area.x, area.y)`.
    pub fn rect(area: Rect, payload: impl Into<CompactString>, style: Style) -> Self {
        Cell {
            content: payload.into(),
            style,
            kind: CellKind::Rect(area),
        }
    }

    /// The cell's structural kind.
    #[inline]
    pub fn kind(&self) -> CellKind {
        self.kind
    }

    /// Whether this cell is a single-column glyph cell.
    #[inline]
    pub fn is_narrow(&self) -> bool {
        matches!(self.kind, CellKind::Narrow)
    }

    /// Whether this cell is the primary of a two-column grapheme.
    #[inline]
    pub fn is_wide(&self) -> bool {
        matches!(self.kind, CellKind::Wide)
    }

    /// Whether this cell is a wide-character continuation placeholder.
    #[inline]
    pub fn is_continuation(&self) -> bool {
        matches!(self.kind, CellKind::Continuation)
    }

    /// Whether this cell is part of a rect (anchor or body).
    #[inline]
    pub fn is_rect(&self) -> bool {
        matches!(self.kind, CellKind::Rect(_))
    }

    /// The cell's grapheme-cluster content. Empty for a wide-cell
    /// continuation placeholder.
    #[inline]
    pub fn content(&self) -> &str {
        self.content.as_str()
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

    /// Column footprint of this cell on the grid.
    ///
    /// - `Narrow` → 1
    /// - `Wide`   → 2
    /// - `Continuation` → 0 (the second slot of a wide primary)
    /// - `Rect(area)`   → `area.width`
    ///
    /// Callers walking a row by cursor advance can use this directly;
    /// hitting a rect cell jumps past the whole rect's footprint.
    #[inline]
    pub fn width(&self) -> u16 {
        match self.kind {
            CellKind::Narrow => 1,
            CellKind::Wide => 2,
            CellKind::Continuation => 0,
            // Rect cells split into two roles distinguished by
            // whether the cell carries content:
            //
            // * Anchor (non-empty content): behaves like a wide
            //   primary spanning `area.width` columns. The payload
            //   bytes are emitted once and the cursor advances by
            //   the full footprint.
            // * Body (empty content): behaves like a continuation
            //   cell — width 0, no output, skipped by the diff.
            CellKind::Rect(area) => {
                if matches!(self.content.as_str(), "") {
                    0
                } else {
                    area.width
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blank_cell() {
        let c = Cell::BLANK;
        assert!(c.is_narrow());
        assert_eq!(c, Cell::BLANK);
        assert!(c.style().is_empty());
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
        assert_ne!(c, Cell::BLANK);
    }

    #[test]
    fn test_rect_cell() {
        let r = Rect::new(2, 3, 4, 5);
        let c = Cell::rect(r, "\x1bPq\x1b\\", Style::EMPTY);
        assert!(c.is_rect());
        assert_eq!(c.kind(), CellKind::Rect(r));
        assert_eq!(c.width(), 4);
        assert_ne!(c, Cell::BLANK);
    }

    #[test]
    fn rect_body_reports_zero_width() {
        let r = Rect::new(0, 0, 4, 4);
        let body = Cell {
            content: CompactString::default(),
            style: Style::EMPTY,
            kind: CellKind::Rect(r),
        };
        assert!(body.is_rect());
        assert_eq!(body.width(), 0);
        assert_ne!(body, Cell::BLANK);
    }

    #[test]
    fn test_cell_with_style() {
        let c = Cell::narrow("x").with_style(Style::EMPTY.with_bold());
        assert!(c.style().attrs.contains(crate::style::AttrFlags::BOLD));
    }
}

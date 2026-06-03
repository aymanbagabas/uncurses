pub mod segment;

pub use segment::graphemes;

use compact_str::CompactString;

use crate::layout::Rect;
use crate::style::Style;

/// Shape of a cell within the grid.
///
/// Replaces the old `width: u8` field with a typed enum so wide-cell
/// invariants and richer multi-cell content can be expressed
/// structurally rather than as magic numbers.
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
    /// A cell that belongs to a rich-content rectangle —
    /// addon-managed content the renderer must treat as opaque
    /// (e.g. a raster image painted via a control sequence). The
    /// cell at `rect.x, rect.y` is the *anchor* and carries the
    /// payload bytes in [`Cell::content`]; cells elsewhere in
    /// `rect` are *body* cells with empty content. The renderer
    /// emits the anchor's content verbatim, leaves body cells
    /// untouched, and disables scroll / ICH / DCH on rows
    /// containing rect cells.
    Rect(Rect),
}

impl CellKind {
    /// Display-width contribution of a cell with this kind:
    /// 1 for [`Narrow`] and [`Rect`], 2 for [`Wide`], 0 for
    /// [`Continuation`].
    #[inline]
    pub fn width(&self) -> u8 {
        match self {
            CellKind::Narrow => 1,
            CellKind::Wide => 2,
            CellKind::Continuation => 0,
            CellKind::Rect(_) => 1,
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

    /// Create the *anchor* cell of a rich-content rectangle. The
    /// `content` is the payload the renderer emits verbatim when the
    /// rect comes into view (e.g. a sixel DCS or an iTerm2 OSC
    /// 1337). Body cells are stamped via [`Cell::rect_body`] at
    /// every other position inside `rect`.
    pub fn rect_anchor(rect: Rect, content: impl Into<CompactString>) -> Self {
        Cell {
            content: content.into(),
            style: Style::EMPTY,
            kind: CellKind::Rect(rect),
        }
    }

    /// Create a *body* cell of a rich-content rectangle. Body cells
    /// carry no content; their kind alone marks the area as opaque
    /// to the differ. The matching anchor is at `rect.x, rect.y`.
    pub fn rect_body(rect: Rect) -> Self {
        Cell {
            content: CompactString::const_new(""),
            style: Style::EMPTY,
            kind: CellKind::Rect(rect),
        }
    }

    /// Whether this cell is a blank/space.
    pub fn is_blank(&self) -> bool {
        if matches!(self.kind, CellKind::Continuation) {
            return true;
        }
        if matches!(self.kind, CellKind::Rect(_)) {
            // Rect cells are never blank: even body cells are
            // opaque to the differ and must not be treated as
            // overwritable space.
            return false;
        }
        self.content.is_empty() || self.content == " "
    }

    /// Whether this is a wide-char continuation cell.
    #[inline]
    pub fn is_continuation(&self) -> bool {
        matches!(self.kind, CellKind::Continuation)
    }

    /// Whether this cell is part of a [`CellKind::Rect`]
    /// (anchor or body).
    #[inline]
    pub fn is_rect(&self) -> bool {
        matches!(self.kind, CellKind::Rect(_))
    }

    /// Returns the rectangle this cell belongs to, if any.
    #[inline]
    pub fn rect(&self) -> Option<Rect> {
        match self.kind {
            CellKind::Rect(r) => Some(r),
            _ => None,
        }
    }

    /// Whether this cell is the *anchor* of its rectangle (the cell
    /// whose grid position is `(rect.x, rect.y)`). A rect anchor
    /// carries the payload bytes the renderer emits; a rect body
    /// is everything else inside the rect.
    ///
    /// Returns `false` for non-rect cells.
    #[inline]
    pub fn is_rect_anchor_at(&self, x: u16, y: u16) -> bool {
        match self.kind {
            CellKind::Rect(r) => r.x == x && r.y == y,
            _ => false,
        }
    }

    /// Whether this cell is a rect *body* — a rect cell at a position
    /// other than its anchor. Returns `false` for non-rect cells.
    #[inline]
    pub fn is_rect_body_at(&self, x: u16, y: u16) -> bool {
        match self.kind {
            CellKind::Rect(r) => !(r.x == x && r.y == y),
            _ => false,
        }
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

    #[test]
    fn rect_anchor_carries_content_and_rect() {
        let r = Rect {
            x: 3,
            y: 4,
            width: 5,
            height: 2,
        };
        let c = Cell::rect_anchor(r, "\x1bPpayload\x1b\\");
        assert_eq!(c.kind(), &CellKind::Rect(r));
        assert_eq!(c.content(), "\x1bPpayload\x1b\\");
        assert_eq!(c.width(), 1);
        assert_eq!(c.rect(), Some(r));
        assert!(c.is_rect());
        assert!(c.is_rect_anchor_at(3, 4));
        assert!(!c.is_rect_body_at(3, 4));
        assert!(!c.is_blank());
    }

    #[test]
    fn rect_body_has_empty_content_and_position_predicates() {
        let r = Rect {
            x: 3,
            y: 4,
            width: 5,
            height: 2,
        };
        let c = Cell::rect_body(r);
        assert_eq!(c.kind(), &CellKind::Rect(r));
        assert_eq!(c.content(), "");
        assert_eq!(c.width(), 1);
        assert!(c.is_rect());
        assert!(!c.is_rect_anchor_at(4, 4));
        assert!(c.is_rect_body_at(4, 4));
        assert!(c.is_rect_body_at(7, 5));
        // A rect cell is never "blank" — it owns the area.
        assert!(!c.is_blank());
    }

    #[test]
    fn non_rect_cells_have_no_rect() {
        assert_eq!(Cell::BLANK.rect(), None);
        assert_eq!(Cell::narrow("a").rect(), None);
        assert_eq!(Cell::wide("あ").rect(), None);
        assert_eq!(Cell::continuation().rect(), None);
        assert!(!Cell::narrow("a").is_rect());
        assert!(!Cell::narrow("a").is_rect_anchor_at(0, 0));
        assert!(!Cell::narrow("a").is_rect_body_at(0, 0));
    }
}

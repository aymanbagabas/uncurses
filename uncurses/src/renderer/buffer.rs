//! RenderBuffer — a Buffer with per-line dirty tracking.

use crate::cell::{Cell, CellKind};
use crate::layout::{Position, Rect};

use crate::buffer::{Bounded, Buffer, Surface, SurfaceMut};

/// Tracks which span of cells on a line has been modified.
#[derive(Debug, Clone, Copy)]
pub struct TouchedSpan {
    pub first: u16,
    pub last: u16,
}

/// A buffer that tracks which lines/cells have been modified since last render.
#[derive(Debug, Clone)]
pub struct RenderBuffer {
    pub buffer: Buffer,
    touched: Vec<Option<TouchedSpan>>,
}

impl RenderBuffer {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            buffer: Buffer::new(width, height),
            touched: vec![None; height as usize],
        }
    }

    pub fn width(&self) -> u16 {
        self.buffer.width()
    }

    pub fn height(&self) -> u16 {
        self.buffer.height()
    }

    /// Mark a line as touched, expanding the span to include `pos.x`.
    #[allow(dead_code)]
    pub fn touch(&mut self, pos: impl Into<Position>) {
        let pos = pos.into();
        let y = pos.y as usize;
        if y >= self.touched.len() {
            return;
        }
        match &mut self.touched[y] {
            Some(span) => {
                span.first = span.first.min(pos.x);
                span.last = span.last.max(pos.x);
            }
            None => {
                self.touched[y] = Some(TouchedSpan {
                    first: pos.x,
                    last: pos.x,
                });
            }
        }
    }

    /// Mark a range of columns on a line as touched. `y` is the 0-based row.
    pub fn touch_line(&mut self, y: u16, first: u16, last: u16) {
        let y = y as usize;
        if y >= self.touched.len() {
            return;
        }
        match &mut self.touched[y] {
            Some(span) => {
                span.first = span.first.min(first);
                span.last = span.last.max(last);
            }
            None => {
                self.touched[y] = Some(TouchedSpan { first, last });
            }
        }
    }

    /// Mark an entire line as touched. `y` is the 0-based row.
    pub fn touch_full_line(&mut self, y: u16) {
        if self.width() > 0 {
            self.touch_line(y, 0, self.width() - 1);
        }
    }

    /// Set a cell and mark it as touched if changed.
    pub fn set_cell(&mut self, pos: impl Into<Position>, cell: &Cell) {
        let pos = pos.into();
        let existing = self.buffer.cell(pos);
        let changed = existing.is_none_or(|e| e != cell);

        if changed {
            let new_width = cell.width().max(1);
            let prev_width = existing.map(|e| e.width()).unwrap_or(0).max(1);
            let width = new_width.max(prev_width);
            // Capture rect identities the underlying `Buffer::set`
            // is about to rewrite (the departing old rect and the
            // incoming new rect) so every row they span is touched
            // — body rows would otherwise stay unvisited by the
            // per-row diff and the screen would keep stale pixels.
            let outgoing_rect = match existing.map(Cell::kind) {
                Some(CellKind::Rect(area)) => Some(area),
                _ => None,
            };
            let incoming_rect = match cell.kind() {
                CellKind::Rect(area) if pos.x == area.x && pos.y == area.y => Some(area),
                _ => None,
            };
            self.buffer.set(pos, cell);
            let end_col = pos.x + width - 1;
            self.touch_line(pos.y, pos.x, end_col);

            if let Some(old) = outgoing_rect
                && incoming_rect.is_some_and(|new| new != old)
            {
                self.touch_rect_rows(old);
            }
            if let Some(new) = incoming_rect {
                self.touch_rect_rows(new);
            }
        }
    }

    /// Touch every row that `area` covers, clipped to the buffer.
    fn touch_rect_rows(&mut self, area: Rect) {
        let bounds = self.buffer.bounds();
        let clipped = bounds.intersection(area);
        if clipped.is_empty() {
            return;
        }
        let last_col = clipped.right().saturating_sub(1);
        for y in clipped.top()..clipped.bottom() {
            self.touch_line(y, clipped.left(), last_col);
        }
    }

    /// Get the touched span for a line, if any. `y` is the 0-based row.
    pub fn touched(&self, y: u16) -> Option<TouchedSpan> {
        self.touched.get(y as usize).copied().flatten()
    }

    /// Whether any line has been touched.
    pub fn has_changes(&self) -> bool {
        self.touched.iter().any(|t| t.is_some())
    }

    /// Count the number of touched lines.
    #[allow(dead_code)]
    pub fn touched_line_count(&self) -> usize {
        self.touched.iter().filter(|t| t.is_some()).count()
    }

    /// Clear all touched flags.
    pub fn clear_touched(&mut self) {
        for t in &mut self.touched {
            *t = None;
        }
    }

    /// Mark all lines as touched (force full redraw).
    pub fn touch_all(&mut self) {
        let width = self.width();
        for y in 0..self.height() {
            self.touch_line(y, 0, width.saturating_sub(1));
        }
    }

    /// Resize the buffer, marking everything as touched.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.buffer.resize(width, height);
        self.touched.resize(height as usize, None);
        self.touch_all();
    }

    /// Get line reference. `y` is the 0-based row.
    pub fn line(&self, y: u16) -> Option<&[Cell]> {
        self.buffer.line(y)
    }

    /// Get mutable line reference. `y` is the 0-based row. Does **not**
    /// mark the line as touched; the caller is responsible for tracking
    /// touch state when bypassing [`Self::set_cell`].
    pub fn line_mut(&mut self, y: u16) -> Option<&mut [Cell]> {
        self.buffer.line_mut(y)
    }

    /// Mutable reference to the cell at `pos`, marking the column
    /// span covered by the cell as touched. Returns `None` for
    /// out-of-bounds positions.
    ///
    /// Unlike [`Self::set_cell`], this does not compare against the
    /// existing cell — the touched span is extended unconditionally and
    /// [`crate::renderer::Renderer::sync_front`] filters unchanged
    /// cells by reference equality. Callers must not change
    /// [`Cell::width`] through this handle; use [`Self::set_cell`] for
    /// wide-cell writes that need continuation-column accounting.
    pub fn cell_mut(&mut self, pos: impl Into<Position>) -> Option<&mut Cell> {
        let pos = pos.into();
        if pos.y >= self.buffer.height() || pos.x >= self.buffer.width() {
            return None;
        }
        // Inspect width before the mutable borrow so wide cells touch
        // their continuation column. Width stays stable under the
        // documented contract (no width changes via this handle).
        let width = self.buffer.cell(pos).map(|c| c.width().max(1)).unwrap_or(1);
        let end_col = pos.x + width - 1;
        self.touch_line(pos.y, pos.x, end_col);
        self.buffer.cell_mut(pos)
    }

    /// Iterate over touched lines with their row indices (0-based).
    #[allow(dead_code)]
    pub fn touched_lines(&self) -> impl Iterator<Item = (u16, TouchedSpan)> + '_ {
        self.touched
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.map(|span| (i as u16, span)))
    }

    /// Insert `n` lines at `y`. Freed rows are filled with `fill`. All
    /// rows in `[y, bounds_bottom)` are marked touched across the full
    /// width so the renderer redraws them on the next frame.
    pub fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.buffer.insert_lines(y, n, bounds_bottom, fill);
        let bottom = bounds_bottom.min(self.height());
        if y >= bottom || self.width() == 0 {
            return;
        }
        let last_col = self.width() - 1;
        for row in y..bottom {
            self.touch_line(row, 0, last_col);
        }
    }

    /// Delete `n` lines at `y`. Freed rows are filled with `fill`. All
    /// rows in `[y, bounds_bottom)` are marked touched across the full
    /// width so the renderer redraws them on the next frame.
    pub fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.buffer.delete_lines(y, n, bounds_bottom, fill);
        let bottom = bounds_bottom.min(self.height());
        if y >= bottom || self.width() == 0 {
            return;
        }
        let last_col = self.width() - 1;
        for row in y..bottom {
            self.touch_line(row, 0, last_col);
        }
    }

    /// Insert `n` cells at `pos`, touching the line. Delegates to
    /// [`Buffer::insert_cells`] and tracks the touch. Freed cells are
    /// filled with `fill`.
    #[allow(dead_code)]
    pub fn insert_cells(
        &mut self,
        pos: impl Into<Position>,
        n: u16,
        bounds_right: u16,
        fill: &Cell,
    ) {
        let pos = pos.into();
        self.buffer.insert_cells(pos, n, bounds_right, fill);
        self.touch_line(
            pos.y,
            pos.x,
            bounds_right.min(self.width()).saturating_sub(1),
        );
    }

    /// Delete `n` cells at `pos`, touching the line. Delegates to
    /// [`Buffer::delete_cells`] and tracks the touch. Freed cells at
    /// the right edge are filled with `fill`.
    #[allow(dead_code)]
    pub fn delete_cells(
        &mut self,
        pos: impl Into<Position>,
        n: u16,
        bounds_right: u16,
        fill: &Cell,
    ) {
        let pos = pos.into();
        self.buffer.delete_cells(pos, n, bounds_right, fill);
        self.touch_line(
            pos.y,
            pos.x,
            bounds_right.min(self.width()).saturating_sub(1),
        );
    }
}

impl Bounded for RenderBuffer {
    fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width(), self.height())
    }
}

impl Surface for RenderBuffer {
    fn cell(&self, pos: Position) -> Option<&Cell> {
        self.buffer.cell(pos)
    }
}

impl SurfaceMut for RenderBuffer {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        RenderBuffer::set_cell(self, pos, cell);
    }

    fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell> {
        RenderBuffer::cell_mut(self, pos)
    }

    /// Bulk-fill via the underlying [`Buffer::fill_rect`] override and
    /// then touch every affected row once. This skips the per-cell
    /// `set_cell` dispatch and the per-cell touched-span widen the
    /// trait default would perform — at the cost of marking unchanged
    /// rows touched on no-op fills, which only causes the transform
    /// pass to re-check rows that didn't change.
    ///
    /// Rect-anchored regions intersecting the fill are blanked by the
    /// underlying [`Buffer::fill_rect`] across their entire footprint;
    /// rows outside `rect` that hold blanked rect bodies are touched
    /// here so the renderer re-syncs them.
    fn fill_rect(&mut self, rect: Rect, cell: &Cell) {
        let clipped = self.bounds().intersection(rect);
        if clipped.is_empty() {
            return;
        }
        // Snapshot rects that will be blanked before they vanish, so we
        // can mark every row they occupy as touched.
        let extra_rects = self.buffer.unique_rects_in(clipped);
        self.buffer.fill_rect(rect, cell);
        let last_col = clipped.right().saturating_sub(1);
        for y in clipped.top()..clipped.bottom() {
            self.touch_line(y, clipped.left(), last_col);
        }
        let buf_bounds = self.buffer.bounds();
        for area in extra_rects {
            let clipped_area = buf_bounds.intersection(area);
            if clipped_area.is_empty() {
                continue;
            }
            let area_last_col = clipped_area.right().saturating_sub(1);
            for y in clipped_area.top()..clipped_area.bottom() {
                self.touch_line(y, clipped_area.left(), area_last_col);
            }
        }
    }

    /// Delegate to [`RenderBuffer::insert_lines`] for the row-swap fast
    /// path and the precise touched-span bookkeeping it performs.
    fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        RenderBuffer::insert_lines(self, y, n, bounds_bottom, fill);
    }

    /// Delegate to [`RenderBuffer::delete_lines`] for the row-swap fast
    /// path and the precise touched-span bookkeeping it performs.
    fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        RenderBuffer::delete_lines(self, y, n, bounds_bottom, fill);
    }

    /// Delegate to [`RenderBuffer::insert_cells`] for the in-place
    /// rotate fast path and the row-touch bookkeeping it performs.
    fn insert_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        RenderBuffer::insert_cells(self, pos, n, bounds_right, fill);
    }

    /// Delegate to [`RenderBuffer::delete_cells`] for the in-place
    /// rotate fast path and the row-touch bookkeeping it performs.
    fn delete_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        RenderBuffer::delete_cells(self, pos, n, bounds_right, fill);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_render_buffer() {
        let rb = RenderBuffer::new(80, 24);
        assert_eq!(rb.width(), 80);
        assert_eq!(rb.height(), 24);
        assert!(!rb.has_changes());
    }

    #[test]
    fn test_set_cell_touches() {
        let mut rb = RenderBuffer::new(10, 5);
        rb.set_cell((3, 2), &Cell::narrow("X"));
        assert!(rb.has_changes());
        assert!(rb.touched(2).is_some());
        assert!(rb.touched(0).is_none());
    }

    #[test]
    fn test_same_cell_no_touch() {
        let mut rb = RenderBuffer::new(10, 5);
        // Set same as blank — should not touch
        rb.set_cell((0, 0), &Cell::BLANK);
        assert!(!rb.has_changes());
    }

    #[test]
    fn test_touched_span_expansion() {
        let mut rb = RenderBuffer::new(10, 5);
        rb.set_cell((2, 0), &Cell::narrow("A"));
        rb.set_cell((7, 0), &Cell::narrow("B"));
        let span = rb.touched(0).unwrap();
        assert_eq!(span.first, 2);
        assert_eq!(span.last, 7);
    }

    #[test]
    fn test_clear_touched() {
        let mut rb = RenderBuffer::new(10, 5);
        rb.set_cell((0, 0), &Cell::narrow("X"));
        assert!(rb.has_changes());
        rb.clear_touched();
        assert!(!rb.has_changes());
    }

    #[test]
    fn test_touch_all() {
        let mut rb = RenderBuffer::new(10, 5);
        rb.touch_all();
        assert_eq!(rb.touched_line_count(), 5);
    }

    #[test]
    fn fill_rect_empty_rect_does_not_touch_lines() {
        let mut rb = RenderBuffer::new(10, 5);
        rb.fill_rect(Rect::new(2, 1, 0, 3), &Cell::BLANK);
        assert!(!rb.has_changes());
        rb.fill_rect(Rect::new(2, 1, 3, 0), &Cell::BLANK);
        assert!(!rb.has_changes());
    }

    #[test]
    fn fill_rect_clipped_to_zero_width_does_not_touch_lines() {
        // Rect starts at/beyond buffer width — after clipping right ==
        // left, so there's nothing to mark dirty.
        let mut rb = RenderBuffer::new(10, 5);
        rb.fill_rect(Rect::new(10, 0, 3, 2), &Cell::BLANK);
        assert!(!rb.has_changes());
    }

    #[test]
    fn fill_rect_via_trait_default_touches_every_affected_row() {
        // `fill_rect` is the default impl on `SurfaceMut`. It must
        // route every write through `set_cell` so dirty tracking sees
        // the change.
        let mut rb = RenderBuffer::new(10, 5);
        rb.fill_rect(Rect::new(2, 1, 3, 2), &Cell::narrow("X"));
        assert!(rb.touched(0).is_none());
        let r1 = rb.touched(1).expect("row 1 should be dirty");
        assert_eq!((r1.first, r1.last), (2, 4));
        let r2 = rb.touched(2).expect("row 2 should be dirty");
        assert_eq!((r2.first, r2.last), (2, 4));
        assert!(rb.touched(3).is_none());
    }

    #[test]
    fn fill_rect_via_trait_default_lays_wide_pairs_without_clobbering() {
        // Default `fill_rect` must step by `cell.width()` — otherwise a
        // wide fill cascades: each set_cell sees the previous cell's
        // continuation marker, walks back, blanks the just-written
        // primary, then writes its own primary.
        let mut rb = RenderBuffer::new(8, 1);
        let wide = Cell::wide("漢");
        rb.fill_rect(Rect::new(0, 0, 6, 1), &wide);
        for x in (0..6).step_by(2) {
            let p = rb.buffer.cell(Position::new(x, 0)).unwrap();
            assert_eq!(p.content(), "漢", "primary at x={x}");
            assert_eq!(p.width(), 2);
            let c = rb.buffer.cell(Position::new(x + 1, 0)).unwrap();
            assert!(c.is_continuation(), "continuation at x={}", x + 1);
        }
    }

    #[test]
    fn fill_rect_via_trait_default_blanks_trailing_partial_slot() {
        // Odd width with a 2-wide fill leaves one trailing slot that
        // can't hold a primary; it must become a blank, not garbage.
        let mut rb = RenderBuffer::new(8, 1);
        let wide = Cell::wide("漢");
        rb.fill_rect(Rect::new(0, 0, 5, 1), &wide);
        assert_eq!(rb.buffer.cell(Position::new(0, 0)).unwrap().width(), 2);
        assert_eq!(rb.buffer.cell(Position::new(2, 0)).unwrap().width(), 2);
        let trailing = rb.buffer.cell(Position::new(4, 0)).unwrap();
        assert!(trailing == &Cell::BLANK, "trailing slot must be blank");
        assert_eq!(trailing.width(), 1);
    }

    #[test]
    fn clear_via_trait_default_marks_every_row() {
        let mut rb = RenderBuffer::new(4, 3);
        // Stage a non-blank background so clear() has real work to do.
        rb.fill_rect(Rect::new(0, 0, 4, 3), &Cell::narrow("X"));
        rb.clear_touched();
        rb.clear();
        for y in 0..3 {
            let span = rb.touched(y).expect("every row should be dirty");
            assert_eq!((span.first, span.last), (0, 3));
        }
    }

    #[test]
    fn fill_rect_width1_clears_orphan_primary_at_left_edge() {
        // A wide cell straddling `lo`: primary at col 2, continuation
        // at col 3. The fill region `[3, 6)` overwrites the
        // continuation but the bulk-fill path must also blank the
        // primary at col 2 so no half-wide cell is left behind.
        let mut rb = RenderBuffer::new(8, 1);
        rb.set_cell((2, 0), &Cell::wide("漢"));
        assert!(
            rb.buffer
                .cell(Position::new(3, 0))
                .unwrap()
                .is_continuation()
        );
        rb.fill_rect(Rect::new(3, 0, 3, 1), &Cell::BLANK);
        assert!(
            rb.buffer.cell(Position::new(2, 0)).unwrap() == &Cell::BLANK,
            "left-straddle primary must be blanked",
        );
        for x in 3..6 {
            assert!(rb.buffer.cell(Position::new(x, 0)).unwrap() == &Cell::BLANK);
        }
    }

    #[test]
    fn fill_rect_width1_clears_orphan_continuation_at_right_edge() {
        // A wide cell straddling `hi - 1`: primary at col 5,
        // continuation at col 6. The fill region `[3, 6)` overwrites
        // the primary but the bulk-fill path must also blank the
        // continuation at col 6 sitting just past `hi`.
        let mut rb = RenderBuffer::new(8, 1);
        rb.set_cell((5, 0), &Cell::wide("漢"));
        assert!(
            rb.buffer
                .cell(Position::new(6, 0))
                .unwrap()
                .is_continuation()
        );
        rb.fill_rect(Rect::new(3, 0, 3, 1), &Cell::BLANK);
        for x in 3..6 {
            assert!(rb.buffer.cell(Position::new(x, 0)).unwrap() == &Cell::BLANK);
        }
        assert!(
            !rb.buffer
                .cell(Position::new(6, 0))
                .unwrap()
                .is_continuation(),
            "right-straddle orphan continuation must be cleared",
        );
        assert!(rb.buffer.cell(Position::new(6, 0)).unwrap() == &Cell::BLANK);
    }

    #[test]
    fn set_cell_with_rect_anchor_touches_every_row_of_area() {
        let mut rb = RenderBuffer::new(20, 6);
        let area = Rect::new(4, 1, 5, 3);
        rb.set_cell((4, 1), &Cell::rect(area, "DCS", crate::style::Style::EMPTY));

        for y in 1..4 {
            let span = rb.touched(y).expect("row {y} should be touched");
            assert!(span.first <= 4 && span.last >= 8);
        }
        assert!(rb.touched(0).is_none());
        assert!(rb.touched(4).is_none());
    }

    #[test]
    fn set_cell_replacing_rect_with_different_rect_touches_both_footprints() {
        let mut rb = RenderBuffer::new(20, 8);
        let old = Rect::new(2, 1, 4, 3);
        let new = Rect::new(2, 1, 3, 5);
        rb.set_cell((2, 1), &Cell::rect(old, "OLD", crate::style::Style::EMPTY));
        rb.clear_touched();

        rb.set_cell((2, 1), &Cell::rect(new, "NEW", crate::style::Style::EMPTY));

        for y in 1..6 {
            assert!(rb.touched(y).is_some(), "row {y} should be touched");
        }
    }
}

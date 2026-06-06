//! RenderBuffer — a Buffer with per-line dirty tracking.

use crate::cell::Cell;
use crate::layout::{Position, Rect};

use crate::buffer::{Bounded, Buffer, Surface, SurfaceMut};

/// Tracks which span of cells on a line has been modified.
#[derive(Debug, Clone, Copy)]
pub struct TouchedSpan {
    pub first: u16,
    pub last: u16,
}

/// Sentinel value stored in [`RenderBuffer::last_skip_col`] to mark
/// a row that carries no [`Cell::skip`] placeholder.
const NO_SKIP: i16 = -1;

/// A buffer that tracks which lines/cells have been modified since last render.
#[derive(Debug, Clone)]
pub struct RenderBuffer {
    pub buffer: Buffer,
    touched: Vec<Option<TouchedSpan>>,
    /// Per-row column of the rightmost [`Cell::skip`] placeholder,
    /// or [`NO_SKIP`] (`-1`) if the row has none. Maintained in
    /// O(1) for adds and for removes that aren't the recorded
    /// rightmost; only removing the rightmost skip in a row
    /// triggers a bounded rescan to locate the new rightmost.
    ///
    /// The renderer uses this to refuse cell-shifting optimizations
    /// (ICH/DCH) only when the shift's left edge is at or before
    /// the rightmost skip — operations strictly to the right of
    /// every skip on the row stay eligible.
    ///
    /// The signed `i16` representation lets us encode "no skip"
    /// without an `Option` discriminant, halving the per-row
    /// footprint. Cell columns max out at `u16` widths well below
    /// `i16::MAX`, so the sign is free.
    last_skip_col: Vec<i16>,
}

impl RenderBuffer {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            buffer: Buffer::new(width, height),
            touched: vec![None; height as usize],
            last_skip_col: vec![NO_SKIP; height as usize],
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
            let new_width = cell.width().max(1) as u16;
            let prev_width = existing.map(|e| e.width()).unwrap_or(0).max(1) as u16;
            let width = new_width.max(prev_width);
            let prev_skip = existing.is_some_and(Cell::is_skip);
            let new_skip = cell.is_skip();
            self.buffer.set(pos, cell);
            // Touch the range of columns this cell occupies, extending
            // to cover any wider cell being overwritten so the touched
            // span includes the orphaned continuation column(s).
            let end_col = pos.x + width - 1;
            self.touch_line(pos.y, pos.x, end_col);
            if prev_skip != new_skip {
                self.update_last_skip_for_toggle(pos, prev_skip, new_skip);
            }
        }
    }

    /// Update the per-row rightmost-skip-column slot after a single
    /// cell at `pos` toggled its skip kind.
    fn update_last_skip_for_toggle(&mut self, pos: Position, prev_skip: bool, new_skip: bool) {
        let y = pos.y as usize;
        if y >= self.last_skip_col.len() {
            return;
        }
        if new_skip {
            // Adding a skip: extend the rightmost only when this
            // column is past the current one.
            let col = pos.x as i16;
            if self.last_skip_col[y] < col {
                self.last_skip_col[y] = col;
            }
        } else if prev_skip && self.last_skip_col[y] == pos.x as i16 {
            // Removing the recorded rightmost: rescan the columns
            // strictly to its left for the new rightmost. Removals
            // of any other skip leave the rightmost unchanged.
            self.last_skip_col[y] = self.find_last_skip_before(pos.y, pos.x);
        }
    }

    /// Scan row `y` for the rightmost [`Cell::skip`] whose column
    /// is strictly less than `before_col`, returning its column or
    /// [`NO_SKIP`].
    fn find_last_skip_before(&self, y: u16, before_col: u16) -> i16 {
        let Some(line) = self.buffer.line(y) else {
            return NO_SKIP;
        };
        let upper = (before_col as usize).min(line.len());
        line[..upper]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, c)| c.is_skip().then_some(i as i16))
            .unwrap_or(NO_SKIP)
    }

    /// Column of the rightmost [`Cell::skip`] on row `y`, or `None`
    /// when the row has none. O(1).
    pub fn last_skip_col(&self, y: u16) -> Option<u16> {
        let v = *self.last_skip_col.get(y as usize)?;
        (v >= 0).then_some(v as u16)
    }

    /// True when any row in the buffer contains at least one
    /// [`Cell::skip`]. O(rows). Used by the scroll-optimization
    /// gate: scrolling rows that contain skip placeholders would
    /// move external paint pixels out from under their footprint.
    pub fn has_any_skip(&self) -> bool {
        self.last_skip_col.iter().any(|v| *v >= 0)
    }

    /// Recount the rightmost-skip column for row `y` from the
    /// underlying buffer. Used after bulk paths that rewrite a row
    /// without going through [`Self::set_cell`].
    fn recount_skip_row(&mut self, y: u16) {
        let last = self
            .buffer
            .line(y)
            .and_then(|cells| {
                cells
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(i, c)| c.is_skip().then_some(i as i16))
            })
            .unwrap_or(NO_SKIP);
        if let Some(slot) = self.last_skip_col.get_mut(y as usize) {
            *slot = last;
        }
    }

    /// Update the per-row rightmost-skip slot for row `y` after a
    /// bulk fill of columns `[left, last_col]` with a cell whose
    /// skip kind is `fill_skip`.
    ///
    /// The fast path covers the common case where the fill cell
    /// isn't a skip and the existing rightmost skip lies outside
    /// the filled range — the recorded rightmost can't have moved,
    /// so no scan is needed. Only filling over the recorded
    /// rightmost with a non-skip forces a bounded rescan.
    fn update_last_skip_for_fill(&mut self, y: u16, left: u16, last_col: u16, fill_skip: bool) {
        let cur = match self.last_skip_col.get(y as usize) {
            Some(&v) => v,
            None => return,
        };
        let new = if fill_skip {
            // Filling with skip cells cannot shrink the rightmost;
            // it can only push it to the right edge of the fill.
            cur.max(last_col as i16)
        } else if cur < left as i16 || cur > last_col as i16 {
            // Rightmost sits outside the filled range — unchanged.
            return;
        } else {
            // Rightmost was overwritten with non-skip cells; the
            // new rightmost (if any) lives to the left of the fill.
            self.find_last_skip_before(y, left)
        };
        self.last_skip_col[y as usize] = new;
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
        self.last_skip_col.resize(height as usize, NO_SKIP);
        for y in 0..height {
            self.recount_skip_row(y);
        }
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
        let width = self
            .buffer
            .cell(pos)
            .map(|c| c.width().max(1) as u16)
            .unwrap_or(1);
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
            self.recount_skip_row(row);
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
            self.recount_skip_row(row);
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
        self.recount_skip_row(pos.y);
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
        self.recount_skip_row(pos.y);
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
    fn fill_rect(&mut self, rect: Rect, cell: &Cell) {
        let clipped = self.bounds().intersection(rect);
        if clipped.is_empty() {
            return;
        }
        self.buffer.fill_rect(rect, cell);
        let last_col = clipped.right().saturating_sub(1);
        let left = clipped.left();
        let fill_skip = cell.is_skip();
        for y in clipped.top()..clipped.bottom() {
            self.touch_line(y, left, last_col);
        }
        // Full-row fills cover every column, so every existing
        // rightmost-skip column falls inside the fill — collapse
        // the per-row update into a single `slice::fill` (memset).
        if left == 0 && last_col + 1 == self.width() {
            let top = clipped.top() as usize;
            let bottom = clipped.bottom() as usize;
            let value = if fill_skip { last_col as i16 } else { NO_SKIP };
            self.last_skip_col[top..bottom].fill(value);
        } else {
            for y in clipped.top()..clipped.bottom() {
                self.update_last_skip_for_fill(y, left, last_col, fill_skip);
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
        assert!(trailing.is_blank(), "trailing slot must be blank");
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
            rb.buffer.cell(Position::new(2, 0)).unwrap().is_blank(),
            "left-straddle primary must be blanked",
        );
        for x in 3..6 {
            assert!(rb.buffer.cell(Position::new(x, 0)).unwrap().is_blank());
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
            assert!(rb.buffer.cell(Position::new(x, 0)).unwrap().is_blank());
        }
        assert!(
            !rb.buffer
                .cell(Position::new(6, 0))
                .unwrap()
                .is_continuation(),
            "right-straddle orphan continuation must be cleared",
        );
        assert!(rb.buffer.cell(Position::new(6, 0)).unwrap().is_blank());
    }
}

//! Touch-tracked render buffer.
//!
//! [`RenderBuffer`] wraps the core cell [`Buffer`] with per-row touched
//! spans. Drawing code can update only the cells it changed, while the
//! renderer later visits touched spans and compares them against tracked
//! terminal state before emitting bytes.

use crate::cell::Cell;
use crate::layout::{Position, Rect};
use crate::renderer::packed::Ref;

use std::sync::Arc;

use crate::buffer::{Bounded, Grid, Surface, SurfaceMut};
use crate::renderer::packed::arena::{self, Arena};

/// Inclusive modified-column span for one row.
///
/// `first` and `last` are zero-based columns. A row with no touches is
/// represented by `None` in the owning [`RenderBuffer`], not by an empty
/// span.
#[derive(Debug, Clone, Copy)]
pub struct TouchedSpan {
    /// Leftmost touched column.
    pub first: u16,
    /// Rightmost touched column.
    pub last: u16,
}

/// Cell buffer with per-line dirty tracking for renderer diffs.
///
/// # Model
///
/// The embedded [`Buffer`] holds cell contents. The `touched` table says
/// which inclusive column span changed on each row since the last clear.
/// Mutating helpers update both structures; read-only helpers leave touch
/// state unchanged.
#[derive(Debug, Clone)]
pub struct RenderBuffer {
    /// Packed cells. Ids belong to `arena`.
    grid: Grid<Ref>,
    /// Last hyperlink interned through this buffer and the id it got.
    ///
    /// A link spans a run, and the painter hands every cell in that run the
    /// same `Arc`, so the common case is a pointer comparison. That matters
    /// more than the style memo does: interning a link builds an owned
    /// `Link` to probe with, so a miss costs two allocations on top of the
    /// lock.
    last_link: Option<(std::sync::Arc<crate::style::Link>, arena::LinkId)>,
    /// Last style interned through this buffer and the id it got.
    ///
    /// Adjacent cells almost always share a style, so remembering one pair
    /// turns a run into a comparison instead of a lock and a hash probe.
    /// Writes take `&mut self`, so this needs no synchronisation of its own.
    last_style: Option<(crate::style::Style, arena::StyleId)>,
    /// Interning storage the ids in `grid` belong to.
    ///
    /// The renderer's front, back, and tracked buffers all share one arena,
    /// which is what lets the frame diff compare cells as plain integers.
    arena: Arc<dyn Arena>,
    touched: Vec<Option<TouchedSpan>>,
}

impl RenderBuffer {
    /// Create a render buffer with all lines initially untouched.
    ///
    /// # Parameters
    ///
    /// - `width`: number of columns.
    /// - `height`: number of rows.
    ///
    /// # Returns
    ///
    /// A blank buffer of the requested size with no touched rows.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            grid: Grid::new(width, height),
            last_link: None,
            last_style: None,
            arena: arena::global(),
            touched: vec![None; height as usize],
        }
    }

    /// Read the packed cell at `pos`.
    pub(crate) fn cell_ref(&self, pos: impl Into<Position>) -> Option<Ref> {
        self.grid.get(pos.into()).copied()
    }

    /// Pack `cell` into this buffer's arena, reusing the last style id when
    /// it repeats.
    fn intern(&mut self, cell: &Cell) -> Ref {
        // A plain cell needs no arena, and `Cell::intern` already says so.
        if cell.style.link.is_none() && cell.style.style.is_empty() {
            return cell.intern(&*self.arena);
        }
        let style = match self.last_style {
            Some((s, id)) if s == cell.style.style => id,
            _ => {
                let id = self.arena.intern_style(&cell.style.style);
                self.last_style = Some((cell.style.style, id));
                id
            }
        };
        let content = match &cell.content {
            crate::cell::Content::Char(c) => *c as arena::GraphemeId,
            crate::cell::Content::Cluster(s) => self.arena.intern_grapheme(s),
        };
        let link = match &cell.style.link {
            None => arena::EMPTY_LINK,
            // Value equality as well as pointer equality: a caller that
            // rebuilds an equal link per cell still avoids the allocation,
            // and comparing two short strings beats building two.
            // Value equality as well as pointer equality: a caller that
            // rebuilds an equal link per cell still avoids the allocation,
            // and comparing two short strings beats building two.
            Some(l) => match &self.last_link {
                Some((seen, id)) if std::sync::Arc::ptr_eq(seen, l) || seen == l => *id,
                _ => {
                    let id = self.arena.intern_link(&l.url, &l.params);
                    self.last_link = Some((l.clone(), id));
                    id
                }
            },
        };
        Ref::from_ids(content, cell.kind, style, link)
    }

    /// The arena the ids in this buffer belong to.
    pub(crate) fn arena(&self) -> &Arc<dyn Arena> {
        &self.arena
    }

    /// Return the buffer width in cells.
    pub fn width(&self) -> u16 {
        self.grid.width()
    }

    /// Return the buffer height in cells.
    pub fn height(&self) -> u16 {
        self.grid.height()
    }

    /// Mark one cell position as touched.
    ///
    /// Expands the row's existing touched span to include `pos.x`.
    /// Out-of-bounds rows are ignored.
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

    /// Mark an inclusive column range on one row as touched.
    ///
    /// # Parameters
    ///
    /// - `y`: zero-based row.
    /// - `first`: leftmost touched column.
    /// - `last`: rightmost touched column.
    ///
    /// Out-of-bounds rows are ignored. The range is recorded as supplied;
    /// callers should pass in-bounds columns for precise later diffs.
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

    /// Set a cell and mark its occupied columns touched if changed.
    ///
    /// # Parameters
    ///
    /// - `pos`: zero-based destination coordinate.
    /// - `cell`: cell to clone into the buffer.
    ///
    /// # Behavior
    ///
    /// Delegates wide-cell accounting to [`Buffer::set`]. If the new
    /// value equals the existing cell, no touched span is recorded. When
    /// a wide cell is overwritten by a narrower cell, the touched span is
    /// widened to cover stale continuation columns.
    pub(crate) fn set_ref(&mut self, pos: impl Into<Position>, cell: &Ref) {
        let pos = pos.into();
        let existing = self.grid.get(pos).copied();
        let changed = existing.is_none_or(|e| e != *cell);

        if changed {
            let new_width = cell.width().max(1) as u16;
            let prev_width = existing.map(|e| e.width()).unwrap_or(0).max(1) as u16;
            let width = new_width.max(prev_width);
            self.grid.set(pos, cell);
            // Touch the range of columns this cell occupies, extending
            // to cover any wider cell being overwritten so the touched
            // span includes the orphaned continuation column(s).
            let end_col = pos.x + width - 1;
            self.touch_line(pos.y, pos.x, end_col);
        }
    }

    /// Get the touched span for one row.
    ///
    /// # Parameters
    ///
    /// - `y`: zero-based row.
    ///
    /// # Returns
    ///
    /// `Some(TouchedSpan)` when the row has touched columns, otherwise
    /// `None`.
    pub fn touched(&self, y: u16) -> Option<TouchedSpan> {
        self.touched.get(y as usize).copied().flatten()
    }

    /// Return whether any row has touched columns.
    pub fn has_changes(&self) -> bool {
        self.touched.iter().any(|t| t.is_some())
    }

    /// Count the number of touched lines.
    #[allow(dead_code)]
    pub fn touched_line_count(&self) -> usize {
        self.touched.iter().filter(|t| t.is_some()).count()
    }

    /// Clear all touched flags without changing cell contents.
    pub fn clear_touched(&mut self) {
        self.touched.fill(None);
    }

    /// Mark every row as touched across the full width.
    ///
    /// Rows in a zero-width buffer remain untouched because there are no
    /// columns to render.
    pub fn touch_all(&mut self) {
        let width = self.width();
        for y in 0..self.height() {
            self.touch_line(y, 0, width.saturating_sub(1));
        }
    }

    /// Resize the buffer and mark all rows touched.
    ///
    /// # Parameters
    ///
    /// - `width`: new column count.
    /// - `height`: new row count.
    ///
    /// Existing contents are preserved according to [`Buffer::resize`];
    /// newly exposed cells are blank.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.grid.resize(width, height);
        self.touched.resize(height as usize, None);
        self.touch_all();
    }

    /// Borrow one row.
    ///
    /// # Returns
    ///
    /// `Some(&[Cell])` for an in-bounds row, otherwise `None`.
    pub(crate) fn line(&self, y: u16) -> Option<&[Ref]> {
        self.grid.line(y)
    }

    /// Get mutable line reference. `y` is the 0-based row. Does **not**
    /// mark the line as touched; the caller is responsible for tracking
    /// touch state when bypassing [`Self::set_cell`].
    pub(crate) fn line_mut(&mut self, y: u16) -> Option<&mut [Ref]> {
        self.grid.line_mut(y)
    }

    /// Iterate over touched lines with their row indices (0-based).
    #[allow(dead_code)]
    pub fn touched_lines(&self) -> impl Iterator<Item = (u16, TouchedSpan)> + '_ {
        self.touched
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.map(|span| (i as u16, span)))
    }

    /// Insert `n` lines at `y` within a bounded region.
    ///
    /// Freed rows are filled with `fill`. All rows in
    /// `[y, bounds_bottom)` are marked touched across the full width so
    /// the renderer redraws the affected region on the next frame.
    pub(crate) fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Ref) {
        self.grid.insert_lines(y, n, bounds_bottom, fill);
        let bottom = bounds_bottom.min(self.height());
        if y >= bottom || self.width() == 0 {
            return;
        }
        let last_col = self.width() - 1;
        for row in y..bottom {
            self.touch_line(row, 0, last_col);
        }
    }

    /// Delete `n` lines at `y` within a bounded region.
    ///
    /// Freed rows are filled with `fill`. All rows in
    /// `[y, bounds_bottom)` are marked touched across the full width so
    /// the renderer redraws the affected region on the next frame.
    pub(crate) fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Ref) {
        self.grid.delete_lines(y, n, bounds_bottom, fill);
        let bottom = bounds_bottom.min(self.height());
        if y >= bottom || self.width() == 0 {
            return;
        }
        let last_col = self.width() - 1;
        for row in y..bottom {
            self.touch_line(row, 0, last_col);
        }
    }

    /// Insert `n` cells at `pos` within a row-bounded region.
    ///
    /// Delegates to [`Buffer::insert_cells`], fills freed cells with
    /// `fill`, and marks the affected row span touched.
    #[allow(dead_code)]
    pub(crate) fn insert_cells(
        &mut self,
        pos: impl Into<Position>,
        n: u16,
        bounds_right: u16,
        fill: &Ref,
    ) {
        let pos = pos.into();
        self.grid.insert_cells(pos, n, bounds_right, fill);
        self.touch_line(
            pos.y,
            pos.x,
            bounds_right.min(self.width()).saturating_sub(1),
        );
    }

    /// Delete `n` cells at `pos` within a row-bounded region.
    ///
    /// Delegates to [`Buffer::delete_cells`], fills freed right-edge
    /// cells with `fill`, and marks the affected row span touched.
    #[allow(dead_code)]
    pub(crate) fn delete_cells(
        &mut self,
        pos: impl Into<Position>,
        n: u16,
        bounds_right: u16,
        fill: &Ref,
    ) {
        let pos = pos.into();
        self.grid.delete_cells(pos, n, bounds_right, fill);
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
    fn cell(&self, pos: Position) -> Option<Cell> {
        self.grid.get(pos).map(|c| c.resolve(&*self.arena))
    }
}

impl SurfaceMut for RenderBuffer {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        let packed = self.intern(cell);
        self.set_ref(pos, &packed);
    }

    /// Bulk-fill via the underlying [`Buffer::fill_rect`] override and
    /// then touch every affected row once. This skips the per-cell
    /// `set_cell` dispatch and the per-cell touched-span widen the
    /// trait default would perform — at the cost of marking unchanged
    /// rows touched on no-op fills, which only causes the transform
    /// pass to re-check rows that didn't change.
    fn fill_rect(&mut self, rect: Rect, cell: &Cell) {
        let cell = &cell.intern(&*self.arena);
        let clipped = self.bounds().intersection(rect);
        if clipped.is_empty() {
            return;
        }
        self.grid.fill_rect(rect, cell);
        let last_col = clipped.right().saturating_sub(1);
        for y in clipped.top()..clipped.bottom() {
            self.touch_line(y, clipped.left(), last_col);
        }
    }

    /// Delegate to [`RenderBuffer::insert_lines`] for the row-swap fast
    /// path and the precise touched-span bookkeeping it performs.
    fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        let fill = fill.intern(&*self.arena);
        RenderBuffer::insert_lines(self, y, n, bounds_bottom, &fill);
    }

    /// Delegate to [`RenderBuffer::delete_lines`] for the row-swap fast
    /// path and the precise touched-span bookkeeping it performs.
    fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        let fill = fill.intern(&*self.arena);
        RenderBuffer::delete_lines(self, y, n, bounds_bottom, &fill);
    }

    /// Delegate to [`RenderBuffer::insert_cells`] for the in-place
    /// rotate fast path and the row-touch bookkeeping it performs.
    fn insert_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        let fill = fill.intern(&*self.arena);
        RenderBuffer::insert_cells(self, pos, n, bounds_right, &fill);
    }

    /// Delegate to [`RenderBuffer::delete_cells`] for the in-place
    /// rotate fast path and the row-touch bookkeeping it performs.
    fn delete_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        let fill = fill.intern(&*self.arena);
        RenderBuffer::delete_cells(self, pos, n, bounds_right, &fill);
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
        rb.set_ref((3, 2), &Ref::narrow('X'));
        assert!(rb.has_changes());
        assert!(rb.touched(2).is_some());
        assert!(rb.touched(0).is_none());
    }

    #[test]
    fn test_same_cell_no_touch() {
        let mut rb = RenderBuffer::new(10, 5);
        // Set same as blank — should not touch
        rb.set_ref((0, 0), &Ref::BLANK);
        assert!(!rb.has_changes());
    }

    #[test]
    fn test_touched_span_expansion() {
        let mut rb = RenderBuffer::new(10, 5);
        rb.set_ref((2, 0), &Ref::narrow('A'));
        rb.set_ref((7, 0), &Ref::narrow('B'));
        let span = rb.touched(0).unwrap();
        assert_eq!(span.first, 2);
        assert_eq!(span.last, 7);
    }

    #[test]
    fn test_clear_touched() {
        let mut rb = RenderBuffer::new(10, 5);
        rb.set_ref((0, 0), &Ref::narrow('X'));
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
        rb.fill_rect(Rect::new(2, 1, 0, 3), &Cell::default());
        assert!(!rb.has_changes());
        rb.fill_rect(Rect::new(2, 1, 3, 0), &Cell::default());
        assert!(!rb.has_changes());
    }

    #[test]
    fn fill_rect_clipped_to_zero_width_does_not_touch_lines() {
        // Rect starts at/beyond buffer width — after clipping right ==
        // left, so there's nothing to mark dirty.
        let mut rb = RenderBuffer::new(10, 5);
        rb.fill_rect(Rect::new(10, 0, 3, 2), &Cell::default());
        assert!(!rb.has_changes());
    }

    #[test]
    fn fill_rect_via_trait_default_touches_every_affected_row() {
        // `fill_rect` is the default impl on `SurfaceMut`. It must
        // route every write through `set_cell` so dirty tracking sees
        // the change.
        let mut rb = RenderBuffer::new(10, 5);
        rb.fill_rect(Rect::new(2, 1, 3, 2), &Cell::from('X'));
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
        let wide = Cell::from('漢');
        rb.fill_rect(Rect::new(0, 0, 6, 1), &wide);
        for x in (0..6).step_by(2) {
            let p = rb.cell(Position::new(x, 0)).unwrap();
            assert_eq!(p.content.char(), Some('漢'), "primary at x={x}");
            assert_eq!(p.width(), 2);
            let c = rb.cell(Position::new(x + 1, 0)).unwrap();
            assert!(c.is_continuation(), "continuation at x={}", x + 1);
        }
    }

    #[test]
    fn fill_rect_via_trait_default_blanks_trailing_partial_slot() {
        // Odd width with a 2-wide fill leaves one trailing slot that
        // can't hold a primary; it must become a blank, not garbage.
        let mut rb = RenderBuffer::new(8, 1);
        let wide = Cell::from('漢');
        rb.fill_rect(Rect::new(0, 0, 5, 1), &wide);
        assert_eq!(rb.cell(Position::new(0, 0)).unwrap().width(), 2);
        assert_eq!(rb.cell(Position::new(2, 0)).unwrap().width(), 2);
        let trailing = rb.cell(Position::new(4, 0)).unwrap();
        assert!(trailing.is_blank(), "trailing slot must be blank");
        assert_eq!(trailing.width(), 1);
    }

    #[test]
    fn clear_via_trait_default_marks_every_row() {
        let mut rb = RenderBuffer::new(4, 3);
        // Stage a non-blank background so clear() has real work to do.
        rb.fill_rect(Rect::new(0, 0, 4, 3), &Cell::from('X'));
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
        rb.set_ref((2, 0), &Ref::wide('漢'));
        assert!(rb.cell(Position::new(3, 0)).unwrap().is_continuation());
        rb.fill_rect(Rect::new(3, 0, 3, 1), &Cell::default());
        assert!(
            rb.cell(Position::new(2, 0)).unwrap().is_blank(),
            "left-straddle primary must be blanked",
        );
        for x in 3..6 {
            assert!(rb.cell(Position::new(x, 0)).unwrap().is_blank());
        }
    }

    #[test]
    fn fill_rect_width1_clears_orphan_continuation_at_right_edge() {
        // A wide cell straddling `hi - 1`: primary at col 5,
        // continuation at col 6. The fill region `[3, 6)` overwrites
        // the primary but the bulk-fill path must also blank the
        // continuation at col 6 sitting just past `hi`.
        let mut rb = RenderBuffer::new(8, 1);
        rb.set_ref((5, 0), &Ref::wide('漢'));
        assert!(rb.cell(Position::new(6, 0)).unwrap().is_continuation());
        rb.fill_rect(Rect::new(3, 0, 3, 1), &Cell::default());
        for x in 3..6 {
            assert!(rb.cell(Position::new(x, 0)).unwrap().is_blank());
        }
        assert!(
            !rb.cell(Position::new(6, 0)).unwrap().is_continuation(),
            "right-straddle orphan continuation must be cleared",
        );
        assert!(rb.cell(Position::new(6, 0)).unwrap().is_blank());
    }
}

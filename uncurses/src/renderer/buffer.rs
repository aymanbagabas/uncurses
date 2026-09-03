//! Touch-tracked render buffer.
//!
//! [`RenderBuffer`] wraps the core cell [`Buffer`] with per-row touched
//! spans. Drawing code can update only the cells it changed, while the
//! renderer later visits touched spans and compares them against tracked
//! terminal state before emitting bytes.

use crate::cell::Cell;
use crate::layout::{Position, Rect};

use crate::buffer::{Bounded, Buffer, Surface, SurfaceMut};

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
    /// Backing cell buffer.
    pub buffer: Buffer,
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
            buffer: Buffer::new(width, height),
            touched: vec![None; height as usize],
        }
    }

    /// Return the buffer width in cells.
    pub fn width(&self) -> u16 {
        self.buffer.width()
    }

    /// Return the buffer height in cells.
    pub fn height(&self) -> u16 {
        self.buffer.height()
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

    /// The column a write at `pos` starts changing.
    ///
    /// That is `pos.x` itself, unless `pos` holds a continuation whose owning
    /// primary lies further left: writing over a continuation blanks that
    /// primary, so the damage starts there. `Buffer::set` walks back over
    /// chained continuations to find it, and the damage has to walk with it,
    /// since stopping one column short leaves the diff blind to half of what
    /// the write changed.
    ///
    /// `Buffer::set` blanks a primary only when the walk lands on a cell wide
    /// enough to own the column, so the damage may only reach back that far
    /// under the same condition. A continuation beside a narrow cell, or one
    /// that reaches the edge, leaves its neighbour untouched and blanks at
    /// most the column written.
    ///
    /// This reads the row as it stands, so callers ask before writing: the
    /// write is what destroys the evidence.
    fn owned_from(&self, pos: Position) -> u16 {
        if pos.x == 0 {
            return pos.x;
        }
        if !self.buffer.cell(pos).is_some_and(Cell::is_continuation) {
            return pos.x;
        }
        let Some(line) = self.buffer.line(pos.y) else {
            return pos.x;
        };
        let mut pc = pos.x - 1;
        while pc > 0 && line[pc as usize].is_continuation() {
            pc -= 1;
        }
        if line[pc as usize].is_wide() {
            pc
        } else {
            pos.x
        }
    }

    /// The last column a write reaching `pos` changes.
    ///
    /// That is `pos.x` itself, unless `pos` holds a wide primary whose
    /// continuation lies past it: writing over a primary blanks the whole
    /// cluster it owns, so a column outside the write changes with it. The
    /// mirror of [`RenderBuffer::owned_from`], answering the same question on
    /// the other side, and needed for the same reason: a span that stops one
    /// column short leaves the diff blind to a cell the write rewrote, and the
    /// stale half of the cluster survives the frame.
    ///
    /// The reach is clamped to the row, because `Buffer::set` truncates a cell
    /// that does not fit and the span may not claim a column the row does not
    /// have. Only a wide cell owns the column to its right, so a narrow cell or
    /// a continuation reaches no further than itself.
    ///
    /// This reads the row as it stands, so callers ask before writing: the
    /// write is what destroys the evidence.
    fn owned_through(&self, pos: Position) -> u16 {
        let Some(line) = self.buffer.line(pos.y) else {
            return pos.x;
        };
        let Some(cell) = line.get(pos.x as usize) else {
            return pos.x;
        };
        if !cell.is_wide() {
            return pos.x;
        }
        pos.x
            .saturating_add(cell.width() as u16 - 1)
            .min(self.width().saturating_sub(1))
    }

    /// The columns a row shift bounded by `bounds_right` actually changes.
    ///
    /// Both ends close over the clusters the shift breaks, the way a write's
    /// do: the shift can pull a continuation out from beside the primary that
    /// owns it, or carry a primary off and leave its continuation behind, and
    /// either half left standing has to be repainted even though it sits
    /// outside the region asked for. The row is read before the shift, which
    /// is what destroys the evidence.
    fn shift_span(&self, pos: Position, bounds_right: u16) -> (u16, u16) {
        let last = bounds_right.min(self.width()).saturating_sub(1);
        (
            self.owned_from(pos),
            self.owned_through(Position::new(last, pos.y)),
        )
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
    /// a wide cell is overwritten by a narrower cell, the touched span
    /// covers the whole cluster being broken, both the column written and
    /// the primary to its left that the write blanks. A write that lands on
    /// a wide neighbour reaches the continuation that neighbour owns, which
    /// can sit past the columns the write itself covers.
    ///
    /// A continuation is placed by the cell that owns it, so [`Buffer::set`]
    /// ignores one arriving on its own and this records no damage for it.
    pub fn set_cell(&mut self, pos: impl Into<Position>, cell: &Cell) {
        let pos = pos.into();
        // The buffer leaves the column as its owner wrote it, so there is
        // nothing for the diff to look at.
        if cell.is_continuation() {
            return;
        }
        // `Buffer::set` ignores a write outside the grid, and the damage has
        // to agree with it. `cell` answers `None` there, which reads as
        // changed, so the span would be recorded for a row nothing wrote to,
        // and `pos.x + width` can overflow reaching it.
        if pos.y >= self.height() || pos.x >= self.width() {
            return;
        }

        let existing = self.buffer.cell(pos);
        let changed = existing.is_none_or(|e| e != cell);

        if changed {
            let new_width = cell.width().max(1) as u16;
            let prev_width = existing.map(|e| e.width()).unwrap_or(0).max(1) as u16;
            let width = new_width.max(prev_width);
            let first_col = self.owned_from(pos);
            // `Buffer::set` truncates a cell that does not fit, so the span
            // must not claim a column the row does not have either. The
            // saturating add is what keeps a row as wide as `u16` can
            // describe from overflowing on the way to that clamp.
            let end_col = pos
                .x
                .saturating_add(width - 1)
                .min(self.width().saturating_sub(1));
            let last_col = self.owned_through(Position::new(end_col, pos.y));
            self.buffer.set(pos, cell);
            self.touch_line(pos.y, first_col, last_col);
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
        self.buffer.resize(width, height);
        self.touched.resize(height as usize, None);
        self.touch_all();
    }

    /// Borrow one row.
    ///
    /// # Returns
    ///
    /// `Some(&[Cell])` for an in-bounds row, otherwise `None`.
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
    /// `Renderer::sync_front` filters unchanged
    /// cells by value equality. Callers must not change
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

    /// Insert `n` lines at `y` within a bounded region.
    ///
    /// Freed rows are filled with `fill`. All rows in
    /// `[y, bounds_bottom)` are marked touched across the full width so
    /// the renderer redraws the affected region on the next frame.
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

    /// Delete `n` lines at `y` within a bounded region.
    ///
    /// Freed rows are filled with `fill`. All rows in
    /// `[y, bounds_bottom)` are marked touched across the full width so
    /// the renderer redraws the affected region on the next frame.
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

    /// Insert `n` cells at `pos` within a row-bounded region.
    ///
    /// Delegates to [`Buffer::insert_cells`], fills freed cells with
    /// `fill`, and marks the affected row span touched. The span closes over
    /// the clusters the shift breaks at either end, which
    /// [`RenderBuffer::shift_span`] works out from the row as it stands.
    #[allow(dead_code)]
    pub fn insert_cells(
        &mut self,
        pos: impl Into<Position>,
        n: u16,
        bounds_right: u16,
        fill: &Cell,
    ) {
        let pos = pos.into();
        let (first_col, last_col) = self.shift_span(pos, bounds_right);
        self.buffer.insert_cells(pos, n, bounds_right, fill);
        self.touch_line(pos.y, first_col, last_col);
    }

    /// Delete `n` cells at `pos` within a row-bounded region.
    ///
    /// Delegates to [`Buffer::delete_cells`], fills freed right-edge
    /// cells with `fill`, and marks the affected row span touched. The span
    /// closes over the clusters the shift breaks at either end, the way
    /// [`RenderBuffer::insert_cells`] does.
    #[allow(dead_code)]
    pub fn delete_cells(
        &mut self,
        pos: impl Into<Position>,
        n: u16,
        bounds_right: u16,
        fill: &Cell,
    ) {
        let pos = pos.into();
        let (first_col, last_col) = self.shift_span(pos, bounds_right);
        self.buffer.delete_cells(pos, n, bounds_right, fill);
        self.touch_line(pos.y, first_col, last_col);
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
        // A fill starting on a continuation makes the buffer walk back to the
        // primary that owns it and blank from there, and a fill ending on a
        // primary blanks the continuation that primary owns, which lies past
        // the rect. Ask each row where its damage really begins and ends
        // before the fill rewrites the evidence.
        // `touch_line` only merges a span into the row's record, so it does not
        // care that the fill has not run yet.
        let last_col = clipped.right().saturating_sub(1);
        for y in clipped.top()..clipped.bottom() {
            let first = self.owned_from(Position::new(clipped.left(), y));
            let last = self.owned_through(Position::new(last_col, y));
            self.touch_line(y, first, last);
        }
        self.buffer.fill_rect(rect, cell);
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

    /// Assert that every column the operation rewrote lies inside `span`.
    ///
    /// The row is compared cell by cell against how it stood beforehand, so
    /// the test does not depend on having guessed which columns move: a span
    /// that misses one the buffer changed leaves that column stale on screen,
    /// and this is what says so.
    fn assert_span_covers_changes(span: TouchedSpan, before: &[Cell], after: &[Cell], what: &str) {
        for (x, (b, a)) in before.iter().zip(after).enumerate() {
            if b == a {
                continue;
            }
            let x = x as u16;
            assert!(
                (span.first..=span.last).contains(&x),
                "{what}: column {x} changed but the recorded span is {}..={}",
                span.first,
                span.last,
            );
        }
    }

    #[test]
    fn test_new_render_buffer() {
        let rb = RenderBuffer::new(80, 24);
        assert_eq!(rb.width(), 80);
        assert_eq!(rb.height(), 24);
        assert!(!rb.has_changes());
    }

    /// A wide primary can sit two or more columns from the continuation
    /// being written over, which `Buffer::resize` and the row shifts both
    /// leave behind. `Buffer::set` walks back to it and blanks from there,
    /// so the damage span has to reach it too.
    #[test]
    fn damage_reaches_a_primary_blanked_through_chained_continuations() {
        let mut rb = RenderBuffer::new(6, 1);
        rb.set_cell((0, 0), &Cell::wide("\u{4e16}"));
        rb.buffer.line_mut(0).unwrap()[2] = Cell::continuation();
        rb.clear_touched();

        let before = rb.buffer.cell(Position::new(0, 0)).unwrap().clone();
        rb.set_cell((2, 0), &Cell::narrow("A"));
        let after = rb.buffer.cell(Position::new(0, 0)).unwrap().clone();
        let span = rb.touched(0).expect("row 0 touched");

        assert_ne!(before, after, "precondition: the primary was blanked");
        assert_eq!(span.first, 0, "damage must reach the primary it blanked");
    }

    /// `Buffer::set` ignores a write outside the grid, so the damage must
    /// too. Without the bounds check the span is recorded for a row nothing
    /// wrote to, and a column near `u16::MAX` overflows computing `end_col`.
    #[test]
    fn a_write_outside_the_grid_records_no_damage() {
        let mut rb = RenderBuffer::new(6, 1);
        // Fill the row with continuations so the back-walk would run if the
        // guard above it ever let an out-of-bounds position through.
        rb.set_cell((0, 0), &Cell::wide("\u{4e16}"));
        for x in 2..6 {
            rb.buffer.line_mut(0).unwrap()[x] = Cell::continuation();
        }
        rb.clear_touched();

        // x past the right edge, y in bounds: the exact shape the review names.
        rb.set_cell((6, 0), &Cell::narrow("A"));
        rb.set_cell((100, 0), &Cell::narrow("A"));
        rb.set_cell((u16::MAX, 0), &Cell::narrow("A"));
        assert!(
            !rb.has_changes(),
            "an out-of-bounds write must change nothing"
        );

        // y past the bottom edge too.
        rb.set_cell((0, 9), &Cell::narrow("A"));
        assert!(!rb.has_changes());
    }

    /// `Buffer::set` truncates a wide cell that does not fit, so the damage
    /// span must not claim the column past it, and the arithmetic reaching
    /// that clamp must survive the widest row a `u16` can describe.
    #[test]
    fn a_span_stops_at_the_last_column_of_the_row() {
        let mut rb = RenderBuffer::new(6, 1);
        rb.set_cell((5, 0), &Cell::wide("\u{4e16}"));
        let span = rb.touched(0).expect("row 0 touched");
        assert_eq!(span.last, 5, "the span must stop at the last column");

        let mut rb = RenderBuffer::new(u16::MAX, 1);
        rb.set_cell((u16::MAX - 1, 0), &Cell::wide("\u{4e16}"));
        let span = rb.touched(0).expect("row 0 touched");
        assert_eq!(span.last, u16::MAX - 1);
    }

    /// The walk back to a primary follows `Buffer::set`'s ownership rule,
    /// which acts only on a wide cell. A continuation nothing owns leaves its
    /// neighbour alone, so the damage may not claim it.
    #[test]
    fn damage_reaches_back_only_to_a_primary_that_owns_the_column() {
        let cases: [(&str, Vec<Cell>, u16); 3] = [
            (
                "a wide owner is reached",
                vec![
                    Cell::wide("\u{4e16}"),
                    Cell::continuation(),
                    Cell::continuation(),
                ],
                0,
            ),
            (
                "a narrow neighbour is left alone",
                vec![
                    Cell::narrow("a"),
                    Cell::continuation(),
                    Cell::continuation(),
                ],
                2,
            ),
            (
                "a continuation reaching the edge owns nothing",
                vec![
                    Cell::continuation(),
                    Cell::continuation(),
                    Cell::continuation(),
                ],
                2,
            ),
        ];
        for (name, cells, want_first) in cases {
            let mut rb = RenderBuffer::new(6, 1);
            for (x, c) in cells.iter().enumerate() {
                rb.buffer.line_mut(0).unwrap()[x] = c.clone();
            }
            rb.clear_touched();
            rb.set_cell((2, 0), &Cell::narrow("A"));
            let span = rb.touched(0).expect("row 0 touched");
            assert_eq!(span.first, want_first, "{name}");
        }
    }

    /// A wide write covers the column to its right, and a wide primary
    /// standing there owns one further out still. `Buffer::set` blanks that
    /// whole cluster, so the damage has to reach past the columns the write
    /// itself covers or the orphaned continuation survives the frame.
    #[test]
    fn damage_reaches_the_continuation_a_wide_neighbour_owns() {
        let mut rb = RenderBuffer::new(8, 1);
        rb.set_cell((3, 0), &Cell::wide("\u{6f22}"));
        rb.clear_touched();

        let before = rb.line(0).expect("row 0").to_vec();
        rb.set_cell((2, 0), &Cell::wide("\u{4e16}"));
        let after = rb.line(0).expect("row 0").to_vec();
        let span = rb.touched(0).expect("row 0 must be dirty");

        assert!(
            before[4].is_continuation(),
            "precondition: the neighbour owned column 4",
        );
        assert!(
            !after[4].is_continuation(),
            "precondition: the write must have blanked it",
        );
        assert_span_covers_changes(span, &before, &after, "a wide write over a wide neighbour");
        assert_eq!(
            (span.first, span.last),
            (2, 4),
            "damage must reach the neighbour's continuation at column 4",
        );
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
    fn fill_rect_damage_reaches_the_primary_it_blanks() {
        // The bulk fill blanks the primary that owns a continuation it
        // overwrites, and that primary can sit left of the rect. The damage
        // has to reach it: recording only the rect leaves the diff blind to a
        // column the write changed, and the stale glyph survives the frame.
        let mut rb = RenderBuffer::new(8, 1);
        rb.set_cell((2, 0), &Cell::wide("漢"));
        rb.clear_touched();

        rb.fill_rect(Rect::new(3, 0, 3, 1), &Cell::BLANK);

        assert!(
            rb.buffer.cell(Position::new(2, 0)).unwrap().is_blank(),
            "precondition: the fill must have blanked the primary",
        );
        let span = rb.touched(0).expect("row 0 must be dirty");
        assert_eq!(
            (span.first, span.last),
            (2, 5),
            "damage must reach the blanked primary at column 2",
        );
    }

    #[test]
    fn fill_rect_damage_reaches_the_continuation_it_blanks() {
        // The bulk fill blanks the continuation owned by a primary it
        // overwrites, and that continuation sits one column past the rect.
        // The rect is not the damage: recording it alone leaves the diff
        // blind to a column the fill rewrote, and the stale half of the
        // cluster survives the frame.
        let mut rb = RenderBuffer::new(8, 1);
        rb.set_cell((5, 0), &Cell::wide("\u{6f22}"));
        rb.clear_touched();

        let before = rb.line(0).expect("row 0").to_vec();
        rb.fill_rect(Rect::new(3, 0, 3, 1), &Cell::BLANK);
        let after = rb.line(0).expect("row 0").to_vec();
        let span = rb.touched(0).expect("row 0 must be dirty");

        assert!(
            before[6].is_continuation(),
            "precondition: the primary at the rect's edge owned column 6",
        );
        assert!(
            !after[6].is_continuation(),
            "precondition: the fill must have blanked it",
        );
        assert_span_covers_changes(span, &before, &after, "a fill ending on a wide primary");
        assert_eq!(
            (span.first, span.last),
            (3, 6),
            "damage must reach the blanked continuation at column 6",
        );
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

    #[test]
    fn insert_cells_damage_reaches_the_primary_it_orphans() {
        // Starting the shift on a continuation drags it out from beside the
        // primary that owns it, leaving a wide cell claiming a column that
        // now holds something else. The primary sits left of the shift and
        // its own bytes never change, so only the damage can tell the diff to
        // repaint it.
        let mut rb = RenderBuffer::new(8, 1);
        rb.set_cell((2, 0), &Cell::wide("\u{6f22}"));
        rb.clear_touched();

        let before = rb.line(0).expect("row 0").to_vec();
        rb.insert_cells((3, 0), 1, 8, &Cell::BLANK);
        let after = rb.line(0).expect("row 0").to_vec();
        let span = rb.touched(0).expect("row 0 must be dirty");

        assert!(
            after[2].is_wide(),
            "precondition: the primary outlives the shift",
        );
        assert!(
            !after[3].is_continuation(),
            "precondition: the shift took its continuation away",
        );
        assert_span_covers_changes(
            span,
            &before,
            &after,
            "an insert starting on a continuation",
        );
        assert_eq!(
            (span.first, span.last),
            (2, 7),
            "damage must reach the orphaned primary at column 2",
        );
    }

    #[test]
    fn insert_cells_damage_reaches_the_continuation_it_orphans() {
        // The far end breaks the same way. A primary at the last column of
        // the region is carried off by the shift while the continuation it
        // owned stays put outside the region, so the damage has to reach one
        // column past `bounds_right`.
        let mut rb = RenderBuffer::new(8, 1);
        rb.set_cell((5, 0), &Cell::wide("\u{6f22}"));
        rb.clear_touched();

        let before = rb.line(0).expect("row 0").to_vec();
        rb.insert_cells((0, 0), 1, 6, &Cell::BLANK);
        let after = rb.line(0).expect("row 0").to_vec();
        let span = rb.touched(0).expect("row 0 must be dirty");

        assert!(
            !after[5].is_wide(),
            "precondition: the shift carried the primary off",
        );
        assert!(
            after[6].is_continuation(),
            "precondition: its continuation stayed behind",
        );
        assert_span_covers_changes(span, &before, &after, "an insert ending on a wide primary");
        assert_eq!(
            (span.first, span.last),
            (0, 6),
            "damage must reach the orphaned continuation at column 6",
        );
    }

    #[test]
    fn delete_cells_damage_reaches_the_primary_it_orphans() {
        // A left shift breaks a cluster at its start exactly as a right one
        // does, and the primary it strands is just as invisible to a diff
        // that only reads the region asked for.
        let mut rb = RenderBuffer::new(8, 1);
        rb.set_cell((2, 0), &Cell::wide("\u{6f22}"));
        rb.clear_touched();

        let before = rb.line(0).expect("row 0").to_vec();
        rb.delete_cells((3, 0), 1, 8, &Cell::BLANK);
        let after = rb.line(0).expect("row 0").to_vec();
        let span = rb.touched(0).expect("row 0 must be dirty");

        assert!(
            after[2].is_wide(),
            "precondition: the primary outlives the shift",
        );
        assert!(
            !after[3].is_continuation(),
            "precondition: the shift took its continuation away",
        );
        assert_span_covers_changes(span, &before, &after, "a delete starting on a continuation");
        assert_eq!(
            (span.first, span.last),
            (2, 7),
            "damage must reach the orphaned primary at column 2",
        );
    }

    #[test]
    fn delete_cells_damage_reaches_the_continuation_it_orphans() {
        // A primary at the last column of the region is pulled left, and the
        // continuation it owned sits outside the region where the shift
        // cannot reach it. Nothing but the damage says that column now holds
        // half of a cluster whose other half moved.
        let mut rb = RenderBuffer::new(8, 1);
        rb.set_cell((5, 0), &Cell::wide("\u{6f22}"));
        rb.clear_touched();

        let before = rb.line(0).expect("row 0").to_vec();
        rb.delete_cells((0, 0), 1, 6, &Cell::BLANK);
        let after = rb.line(0).expect("row 0").to_vec();
        let span = rb.touched(0).expect("row 0 must be dirty");

        assert!(
            after[4].is_wide(),
            "precondition: the shift pulled the primary left",
        );
        assert!(
            !after[5].is_continuation(),
            "precondition: it landed without its continuation",
        );
        assert!(
            after[6].is_continuation(),
            "precondition: the continuation stayed behind",
        );
        assert_span_covers_changes(span, &before, &after, "a delete ending on a wide primary");
        assert_eq!(
            (span.first, span.last),
            (0, 6),
            "damage must reach the orphaned continuation at column 6",
        );
    }
}

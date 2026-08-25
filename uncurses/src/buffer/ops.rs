//! Row and cell insert/delete operations on a [`Grid`].

use crate::layout::Position;

use super::grid::{Grid, GridCell, fill_line_into};

/// Replace row `y` entirely with `fill`. Wide fills are stepped by
/// `fill.width()` so primaries own their continuation slots. Whole-row
/// replacement means we don't need to clean up wide cells in the row
/// being overwritten — they're discarded as the row is rewritten.
fn fill_row<T: GridCell>(buf: &mut Grid<T>, y: u16, fill: &T) {
    if let Some(line) = buf.line_mut(y) {
        fill_line_into(line, fill);
    }
}

/// Fill `[lo, hi)` on row `y` with `fill`, routing through `set()` so
/// wide-cell semantics around the fill region (and any wide cells in
/// the slots being overwritten) are honored. Wide fills are laid down
/// one primary per `fill.width()` slots; any trailing partial slot gets
/// a plain blank.
fn fill_range<T: GridCell>(buf: &mut Grid<T>, y: u16, lo: u16, hi: u16, fill: &T) {
    if lo >= hi {
        return;
    }
    let step = fill.width().max(1) as u16;
    let mut x = lo;
    while x + step <= hi {
        buf.set((x, y), fill);
        x += step;
    }
    while x < hi {
        buf.set((x, y), &T::blank());
        x += 1;
    }
}

impl<T: GridCell> Grid<T> {
    /// Insert rows within a bounded vertical region.
    ///
    /// Existing rows in `[y, bounds_bottom)` are pushed downward by `n`.
    /// Rows pushed past `bounds_bottom` are discarded, and the newly opened
    /// rows starting at `y` are filled with `fill`.
    ///
    /// # Parameters
    ///
    /// - `y`: zero-based row where insertion begins.
    /// - `n`: number of rows to insert.
    /// - `bounds_bottom`: exclusive lower row bound for the affected region;
    ///   clamped to the buffer height.
    /// - `fill`: cell used to fill inserted rows.
    ///
    /// # Returns
    ///
    /// Nothing.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Calls with `y >= bounds_bottom` are no-ops. Whole rows are swapped in
    /// row-major storage, so wide-cell pairs inside moved rows remain
    /// structurally unchanged.
    pub(crate) fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &T) {
        let bottom = bounds_bottom.min(self.height) as usize;
        let y_usize = y as usize;
        let n = n as usize;

        if y_usize >= bottom {
            return;
        }

        // Shift lines down
        let shift_end = bottom;
        let shift_start = (y_usize + n).min(shift_end);

        // Move from bottom to top to avoid overwrites
        for i in (shift_start..shift_end).rev() {
            let src = i - n;
            if src >= y_usize {
                self.swap_rows(i, src);
            }
        }

        // Clear the inserted lines
        let fill_end = y_usize + n.min(shift_end - y_usize);
        for i in y_usize..fill_end {
            fill_row(self, i as u16, fill);
        }
    }

    /// Delete rows within a bounded vertical region.
    ///
    /// Existing rows below the deleted range are pulled upward within
    /// `[y, bounds_bottom)`. The freed rows at the bottom of the region are
    /// filled with `fill`.
    ///
    /// # Parameters
    ///
    /// - `y`: zero-based first row to delete.
    /// - `n`: number of rows to delete.
    /// - `bounds_bottom`: exclusive lower row bound for the affected region;
    ///   clamped to the buffer height.
    /// - `fill`: cell used to fill freed bottom rows.
    ///
    /// # Returns
    ///
    /// Nothing.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Calls with `y >= bounds_bottom` are no-ops. Deleting more rows than
    /// remain in the region is equivalent to deleting through
    /// `bounds_bottom`.
    pub(crate) fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &T) {
        let bottom = bounds_bottom.min(self.height) as usize;
        let y_usize = y as usize;
        let n = n as usize;

        if y_usize >= bottom {
            return;
        }

        let n = n.min(bottom - y_usize);

        // Shift lines up
        for i in y_usize..bottom - n {
            self.swap_rows(i, i + n);
        }

        // Clear the bottom lines
        for i in bottom - n..bottom {
            fill_row(self, i as u16, fill);
        }
    }

    /// Insert cells within one row.
    ///
    /// Cells in `[pos.x, bounds_right)` on `pos.y` are pushed right by `n`.
    /// Cells pushed past `bounds_right` are discarded, and the newly opened
    /// slots starting at `pos.x` are filled with `fill`.
    ///
    /// # Parameters
    ///
    /// - `pos`: row and starting column for insertion.
    /// - `n`: number of cells to insert.
    /// - `bounds_right`: exclusive right column bound for the affected row
    ///   region; clamped to the buffer width.
    /// - `fill`: cell used to fill newly opened slots.
    ///
    /// # Returns
    ///
    /// Nothing.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Calls with `n == 0`, an out-of-bounds row, or `pos.x >= bounds_right`
    /// are no-ops. Width-1 fills are written directly into the freed span.
    /// Wide fills are routed through [`Buffer::set`] so primary and
    /// continuation columns are placed consistently.
    pub(crate) fn insert_cells(
        &mut self,
        pos: impl Into<Position>,
        n: u16,
        bounds_right: u16,
        fill: &T,
    ) {
        let pos = pos.into();
        let y = pos.y as usize;
        let x = pos.x as usize;
        let n = n as usize;
        let right = bounds_right.min(self.width) as usize;

        if y >= self.height() as usize || x >= right || n == 0 {
            return;
        }

        let n = n.min(right - x);

        let needs_wide_fill = fill.is_wide();
        {
            // Shift the row's `[x, right)` window right by `n` via an
            // in-place rotate. This is a `memmove`-shaped operation,
            // so we avoid an O(W-n) loop of `Cell::clone` plus a heap
            // allocation per non-empty hyperlink.
            let line = self.line_mut(pos.y).expect("row in range");
            line[x..right].rotate_right(n);

            if !needs_wide_fill {
                // Width-1 fill: single sweep over the freed region.
                // This overwrites any stray continuation marker the
                // rotate parked here, so no separate blank pre-pass is
                // needed.
                line[x..x + n].fill(fill.clone());
            } else {
                // Wide fill: raw-blank the freed slots so a parked
                // continuation can't fool `fill_range`'s `set()` calls
                // into walking back and clobbering an unrelated
                // primary cell.
                for cell in &mut line[x..x + n] {
                    *cell = T::blank();
                }
            }
        }

        if needs_wide_fill {
            // Wide fill: route through fill_range so primary /
            // continuation pairs are laid down correctly.
            fill_range(self, pos.y, x as u16, (x + n) as u16, fill);
        }
    }

    /// Delete cells within one row.
    ///
    /// Cells in `[pos.x + n, bounds_right)` on `pos.y` are pulled left by
    /// `n`. The freed slots at the right edge of the affected region are
    /// filled with `fill`.
    ///
    /// # Parameters
    ///
    /// - `pos`: row and starting column for deletion.
    /// - `n`: number of cells to delete.
    /// - `bounds_right`: exclusive right column bound for the affected row
    ///   region; clamped to the buffer width.
    /// - `fill`: cell used to fill freed right-edge slots.
    ///
    /// # Returns
    ///
    /// Nothing.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Calls with `n == 0`, an out-of-bounds row, or `pos.x >= bounds_right`
    /// are no-ops. Width-1 fills are written directly into the freed span.
    /// Wide fills are routed through [`Buffer::set`] so primary and
    /// continuation columns are placed consistently.
    pub(crate) fn delete_cells(
        &mut self,
        pos: impl Into<Position>,
        n: u16,
        bounds_right: u16,
        fill: &T,
    ) {
        let pos = pos.into();
        let y = pos.y as usize;
        let x = pos.x as usize;
        let n = n as usize;
        let right = bounds_right.min(self.width) as usize;

        if y >= self.height() as usize || x >= right || n == 0 {
            return;
        }

        let n = n.min(right - x);

        let needs_wide_fill = fill.is_wide();
        {
            // Shift the row's `[x, right)` window left by `n` via an
            // in-place rotate (memmove-shaped). Cleanup happens below
            // when the freed right-edge slots are filled: width-1 fills
            // overwrite parked continuations directly, and wide fills
            // blank before routing through fill_range.
            let line = self.line_mut(pos.y).expect("row in range");
            line[x..right].rotate_left(n);

            if !needs_wide_fill {
                // Width-1 fill: single sweep over the freed right-edge
                // region. Any stray continuation marker parked there by
                // the rotate is overwritten in this same pass.
                line[right - n..right].fill(fill.clone());
            } else {
                // Wide fill: raw-blank the freed slots before routing
                // through fill_range, so its `set()` calls don't walk
                // back into still-live shifted data.
                for cell in &mut line[right - n..right] {
                    *cell = T::blank();
                }
            }
        }

        if needs_wide_fill {
            fill_range(self, pos.y, (right - n) as u16, right as u16, fill);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::layout::Position;

    #[test]
    fn test_insert_lines() {
        let mut buf = Grid::<Cell>::new(5, 5);
        buf.set((0, 0), &Cell::from('A'));
        buf.set((0, 1), &Cell::from('B'));
        buf.set((0, 2), &Cell::from('C'));

        buf.insert_lines(1, 2, 5, &Cell::default());
        assert_eq!(
            buf.get(Position::new(0, 0)).unwrap().content.char(),
            Some('A')
        );
        assert!(buf.get(Position::new(0, 1)).unwrap().is_blank());
        assert!(buf.get(Position::new(0, 2)).unwrap().is_blank());
        assert_eq!(
            buf.get(Position::new(0, 3)).unwrap().content.char(),
            Some('B')
        );
        assert_eq!(
            buf.get(Position::new(0, 4)).unwrap().content.char(),
            Some('C')
        );
    }

    #[test]
    fn test_delete_lines() {
        let mut buf = Grid::<Cell>::new(5, 5);
        buf.set((0, 0), &Cell::from('A'));
        buf.set((0, 1), &Cell::from('B'));
        buf.set((0, 2), &Cell::from('C'));
        buf.set((0, 3), &Cell::from('D'));

        buf.delete_lines(1, 2, 5, &Cell::default());
        assert_eq!(
            buf.get(Position::new(0, 0)).unwrap().content.char(),
            Some('A')
        );
        assert_eq!(
            buf.get(Position::new(0, 1)).unwrap().content.char(),
            Some('D')
        );
        assert!(buf.get(Position::new(0, 2)).unwrap().is_blank());
    }

    #[test]
    fn test_insert_cells() {
        let mut buf = Grid::<Cell>::new(10, 1);
        buf.set((0, 0), &Cell::from('A'));
        buf.set((1, 0), &Cell::from('B'));
        buf.set((2, 0), &Cell::from('C'));

        buf.insert_cells((1, 0), 2, 10, &Cell::default());
        assert_eq!(
            buf.get(Position::new(0, 0)).unwrap().content.char(),
            Some('A')
        );
        assert!(buf.get(Position::new(1, 0)).unwrap().is_blank());
        assert!(buf.get(Position::new(2, 0)).unwrap().is_blank());
        assert_eq!(
            buf.get(Position::new(3, 0)).unwrap().content.char(),
            Some('B')
        );
        assert_eq!(
            buf.get(Position::new(4, 0)).unwrap().content.char(),
            Some('C')
        );
    }

    #[test]
    fn test_delete_cells() {
        let mut buf = Grid::<Cell>::new(10, 1);
        buf.set((0, 0), &Cell::from('A'));
        buf.set((1, 0), &Cell::from('B'));
        buf.set((2, 0), &Cell::from('C'));
        buf.set((3, 0), &Cell::from('D'));

        buf.delete_cells((1, 0), 2, 10, &Cell::default());
        assert_eq!(
            buf.get(Position::new(0, 0)).unwrap().content.char(),
            Some('A')
        );
        assert_eq!(
            buf.get(Position::new(1, 0)).unwrap().content.char(),
            Some('D')
        );
        assert!(buf.get(Position::new(2, 0)).unwrap().is_blank());
    }

    #[test]
    fn insert_cells_blanks_dangling_continuation_when_primary_shifted_off() {
        // A wide cell straddling the truncation boundary must not leave a
        // continuation marker behind once its primary is pushed past the
        // right edge.
        let mut buf = Grid::<Cell>::new(6, 1);
        buf.set((0, 0), &Cell::from('A'));
        // Wide cell at columns 4-5 (primary at 4, continuation at 5).
        buf.set((4, 0), &Cell::from('漢'));
        assert!(buf.get(Position::new(5, 0)).unwrap().is_continuation());

        // Insert 1 cell at col 1: primary at 4 shifts to 5, continuation
        // (originally at 5) falls off. The new cell at col 5 is the wide
        // primary with no continuation room — set() truncates it to BLANK.
        buf.insert_cells((1, 0), 1, 6, &Cell::default());
        assert_eq!(
            buf.get(Position::new(0, 0)).unwrap().content.char(),
            Some('A')
        );
        assert!(buf.get(Position::new(1, 0)).unwrap().is_blank());
        // The wide cell's continuation that used to be at col 5 must not
        // remain as a stale marker.
        assert!(!buf.get(Position::new(5, 0)).unwrap().is_continuation());
    }

    #[test]
    fn delete_cells_blanks_dangling_primary_at_fill_boundary() {
        // A wide cell straddling the fill region's left edge must have
        // its dangling primary cleaned up when the fill writes a blank
        // over its continuation half.
        let mut buf = Grid::<Cell>::new(6, 1);
        buf.set((0, 0), &Cell::from('A'));
        // Wide cell at columns 4-5 (primary at 4, continuation at 5).
        buf.set((4, 0), &Cell::from('漢'));
        assert!(buf.get(Position::new(5, 0)).unwrap().is_continuation());

        // Delete 1 cell at col 0 (the "A"). Cells shift left so the wide
        // pair lands at cols 3-4. The fill at col 5 (now blank from the
        // shift) is written via set(); col 5 was the continuation prior
        // to the shift but is now arbitrary — what we really need to
        // verify is that no orphan continuations leak past the right
        // boundary.
        buf.delete_cells((0, 0), 1, 6, &Cell::default());
        // Last column must be a clean blank, not a stray continuation.
        let last = buf.get(Position::new(5, 0)).unwrap();
        assert!(!last.is_continuation());
        assert!(last.is_blank());
    }

    #[test]
    fn fill_with_wide_cell_steps_by_width() {
        // When the fill cell is itself wide, each primary must own its
        // continuation slot — no orphan primaries from stepping by 1.
        let mut buf = Grid::<Cell>::new(5, 1);
        let wide = Cell::from('漢');

        // Fill via insert_cells with n covering the whole row.
        buf.insert_cells((0, 0), 5, 5, &wide);
        // Cols 0,2 are wide primaries; 1,3 are continuations; 4 is a
        // plain blank (odd trailing slot can't fit another pair).
        assert_eq!(buf.get(Position::new(0, 0)).unwrap().width(), 2);
        assert!(buf.get(Position::new(1, 0)).unwrap().is_continuation());
        assert_eq!(buf.get(Position::new(2, 0)).unwrap().width(), 2);
        assert!(buf.get(Position::new(3, 0)).unwrap().is_continuation());
        assert!(buf.get(Position::new(4, 0)).unwrap().is_blank());
        assert!(!buf.get(Position::new(4, 0)).unwrap().is_continuation());
    }
}

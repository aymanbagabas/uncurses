//! Buffer insert/delete operations.

use crate::cell::Cell;
use crate::layout::Position;

use super::{Buffer, fill_line_into};

/// Replace row `y` entirely with `fill`. Wide fills are stepped by
/// `fill.width()` so primaries own their continuation slots. Whole-row
/// replacement means we don't need to clean up wide cells in the row
/// being overwritten — they're discarded as the row is rewritten.
fn fill_row(buf: &mut Buffer, y: u16, fill: &Cell) {
    if let Some(line) = buf.line_mut(y) {
        fill_line_into(line, fill);
    }
}

/// Fill `[lo, hi)` on row `y` with `fill`, routing through `set()` so
/// wide-cell semantics around the fill region (and any wide cells in
/// the slots being overwritten) are honored. Wide fills are laid down
/// one primary per `fill.width()` slots; any trailing partial slot gets
/// a plain blank.
fn fill_range(buf: &mut Buffer, y: u16, lo: u16, hi: u16, fill: &Cell) {
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
        buf.set((x, y), &Cell::BLANK);
        x += 1;
    }
}

impl Buffer {
    /// Insert `n` lines at `y`, pushing existing lines down.
    /// Lines pushed past the bottom of `bounds_bottom` are lost. `y` is
    /// the 0-based row. Freed rows are filled with `fill`.
    pub fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
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

    /// Delete `n` lines at `y`, pulling existing lines up.
    /// New blank lines appear at the bottom of the area. `y` is the
    /// 0-based row. Freed rows are filled with `fill`.
    pub fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
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

    /// Insert `n` cells at `pos`, pushing cells right. Cells pushed
    /// past `bounds_right` are lost. Freed cells are filled with `fill`.
    pub fn insert_cells(
        &mut self,
        pos: impl Into<Position>,
        n: u16,
        bounds_right: u16,
        fill: &Cell,
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

        let needs_wide_fill = fill.width() > 1;
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
                    *cell = Cell::BLANK;
                }
            }
        }

        if needs_wide_fill {
            // Wide fill: route through fill_range so primary /
            // continuation pairs are laid down correctly.
            fill_range(self, pos.y, x as u16, (x + n) as u16, fill);
        }
    }

    /// Delete `n` cells at `pos`, pulling cells left. Freed cells at
    /// the right edge are filled with `fill`.
    pub fn delete_cells(
        &mut self,
        pos: impl Into<Position>,
        n: u16,
        bounds_right: u16,
        fill: &Cell,
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

        let needs_wide_fill = fill.width() > 1;
        {
            // Shift the row's `[x, right)` window left by `n` via an
            // in-place rotate (memmove-shaped). The wide-cell pair
            // straddling the shift travels together; any wide cell
            // whose primary fell into the deletion region `[x, x+n)`
            // becomes orphaned just as it did under the previous
            // per-cell shift.
            let line = self.line_mut(pos.y).expect("row in range");
            line[x..right].rotate_left(n);

            if !needs_wide_fill {
                // Width-1 fill: single sweep over the freed right-edge
                // region. Any stray continuation marker parked by the
                // rotate is overwritten in this same pass.
                line[right - n..right].fill(fill.clone());
            } else {
                // Wide fill: raw-blank parked continuations before
                // routing through fill_range, so its `set()` calls
                // don't walk back into the still-live shifted data.
                for cell in &mut line[right - n..right] {
                    *cell = Cell::BLANK;
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
    use crate::buffer::Surface;

    #[test]
    fn test_insert_lines() {
        let mut buf = Buffer::new(5, 5);
        buf.set((0, 0), &Cell::new("A", 1));
        buf.set((0, 1), &Cell::new("B", 1));
        buf.set((0, 2), &Cell::new("C", 1));

        buf.insert_lines(1, 2, 5, &Cell::BLANK);
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().content(), "A");
        assert!(buf.cell(Position::new(0, 1)).unwrap().is_blank());
        assert!(buf.cell(Position::new(0, 2)).unwrap().is_blank());
        assert_eq!(buf.cell(Position::new(0, 3)).unwrap().content(), "B");
        assert_eq!(buf.cell(Position::new(0, 4)).unwrap().content(), "C");
    }

    #[test]
    fn test_delete_lines() {
        let mut buf = Buffer::new(5, 5);
        buf.set((0, 0), &Cell::new("A", 1));
        buf.set((0, 1), &Cell::new("B", 1));
        buf.set((0, 2), &Cell::new("C", 1));
        buf.set((0, 3), &Cell::new("D", 1));

        buf.delete_lines(1, 2, 5, &Cell::BLANK);
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().content(), "A");
        assert_eq!(buf.cell(Position::new(0, 1)).unwrap().content(), "D");
        assert!(buf.cell(Position::new(0, 2)).unwrap().is_blank());
    }

    #[test]
    fn test_insert_cells() {
        let mut buf = Buffer::new(10, 1);
        buf.set((0, 0), &Cell::new("A", 1));
        buf.set((1, 0), &Cell::new("B", 1));
        buf.set((2, 0), &Cell::new("C", 1));

        buf.insert_cells((1, 0), 2, 10, &Cell::BLANK);
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().content(), "A");
        assert!(buf.cell(Position::new(1, 0)).unwrap().is_blank());
        assert!(buf.cell(Position::new(2, 0)).unwrap().is_blank());
        assert_eq!(buf.cell(Position::new(3, 0)).unwrap().content(), "B");
        assert_eq!(buf.cell(Position::new(4, 0)).unwrap().content(), "C");
    }

    #[test]
    fn test_delete_cells() {
        let mut buf = Buffer::new(10, 1);
        buf.set((0, 0), &Cell::new("A", 1));
        buf.set((1, 0), &Cell::new("B", 1));
        buf.set((2, 0), &Cell::new("C", 1));
        buf.set((3, 0), &Cell::new("D", 1));

        buf.delete_cells((1, 0), 2, 10, &Cell::BLANK);
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().content(), "A");
        assert_eq!(buf.cell(Position::new(1, 0)).unwrap().content(), "D");
        assert!(buf.cell(Position::new(2, 0)).unwrap().is_blank());
    }

    #[test]
    fn insert_cells_blanks_dangling_continuation_when_primary_shifted_off() {
        // A wide cell straddling the truncation boundary must not leave a
        // continuation marker behind once its primary is pushed past the
        // right edge.
        let mut buf = Buffer::new(6, 1);
        buf.set((0, 0), &Cell::new("A", 1));
        // Wide cell at columns 4-5 (primary at 4, continuation at 5).
        buf.set((4, 0), &Cell::new("漢", 2));
        assert!(buf.cell(Position::new(5, 0)).unwrap().is_continuation());

        // Insert 1 cell at col 1: primary at 4 shifts to 5, continuation
        // (originally at 5) falls off. The new cell at col 5 is the wide
        // primary with no continuation room — set() truncates it to BLANK.
        buf.insert_cells((1, 0), 1, 6, &Cell::BLANK);
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().content(), "A");
        assert!(buf.cell(Position::new(1, 0)).unwrap().is_blank());
        // The wide cell's continuation that used to be at col 5 must not
        // remain as a stale marker.
        assert!(!buf.cell(Position::new(5, 0)).unwrap().is_continuation());
    }

    #[test]
    fn delete_cells_blanks_dangling_primary_at_fill_boundary() {
        // A wide cell straddling the fill region's left edge must have
        // its dangling primary cleaned up when the fill writes a blank
        // over its continuation half.
        let mut buf = Buffer::new(6, 1);
        buf.set((0, 0), &Cell::new("A", 1));
        // Wide cell at columns 4-5 (primary at 4, continuation at 5).
        buf.set((4, 0), &Cell::new("漢", 2));
        assert!(buf.cell(Position::new(5, 0)).unwrap().is_continuation());

        // Delete 1 cell at col 0 (the "A"). Cells shift left so the wide
        // pair lands at cols 3-4. The fill at col 5 (now blank from the
        // shift) is written via set(); col 5 was the continuation prior
        // to the shift but is now arbitrary — what we really need to
        // verify is that no orphan continuations leak past the right
        // boundary.
        buf.delete_cells((0, 0), 1, 6, &Cell::BLANK);
        // Last column must be a clean blank, not a stray continuation.
        let last = buf.cell(Position::new(5, 0)).unwrap();
        assert!(!last.is_continuation());
        assert!(last.is_blank());
    }

    #[test]
    fn fill_with_wide_cell_steps_by_width() {
        // When the fill cell is itself wide, each primary must own its
        // continuation slot — no orphan primaries from stepping by 1.
        let mut buf = Buffer::new(5, 1);
        let wide = Cell::new("漢", 2);

        // Fill via insert_cells with n covering the whole row.
        buf.insert_cells((0, 0), 5, 5, &wide);
        // Cols 0,2 are wide primaries; 1,3 are continuations; 4 is a
        // plain blank (odd trailing slot can't fit another pair).
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().width(), 2);
        assert!(buf.cell(Position::new(1, 0)).unwrap().is_continuation());
        assert_eq!(buf.cell(Position::new(2, 0)).unwrap().width(), 2);
        assert!(buf.cell(Position::new(3, 0)).unwrap().is_continuation());
        assert!(buf.cell(Position::new(4, 0)).unwrap().is_blank());
        assert!(!buf.cell(Position::new(4, 0)).unwrap().is_continuation());
    }
}

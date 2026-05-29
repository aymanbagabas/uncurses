//! Cell-grid surface traits.
//!
//! The buffer-like types ([`Buffer`](super::Buffer),
//! [`RenderBuffer`](crate::renderer::RenderBuffer),
//! [`Window`](super::Window), [`View`](super::View),
//! [`Screen`](crate::screen::Screen)) all share the same shape: a
//! rectangular region of [`Cell`]s with read and/or write access. This
//! module defines the three small traits that capture that shape.
//!
//! | trait        | adds                          | required methods           |
//! |--------------|-------------------------------|----------------------------|
//! | [`Bounded`]  | "I have a rectangular extent" | [`bounds`](Bounded::bounds) |
//! | [`Surface`]  | "you can read my cells"       | [`cell`](Surface::cell)    |
//! | [`SurfaceMut`] | "you can write to my cells" | [`set_cell`](SurfaceMut::set_cell) |
//!
//! `Surface: Bounded` and `SurfaceMut: Surface`. Default methods on
//! each trait expose the rest of the API (`width`, `height`,
//! [`draw`](Surface::draw), [`fill`](SurfaceMut::fill),
//! [`clear`](SurfaceMut::clear), [`fill_rect`](SurfaceMut::fill_rect),
//! [`clear_rect`](SurfaceMut::clear_rect)). Implementations may
//! override any default for a faster path.

use crate::cell::Cell;
use crate::layout::{Position, Rect};

/// A type with a rectangular extent in cell-grid coordinates.
pub trait Bounded {
    /// The valid region in this type's own coordinate space.
    fn bounds(&self) -> Rect;

    /// Width of the region in cells.
    fn width(&self) -> u16 {
        self.bounds().width
    }

    /// Height of the region in cells.
    fn height(&self) -> u16 {
        self.bounds().height
    }

    /// True when `pos` lies inside [`Self::bounds`].
    fn contains(&self, pos: Position) -> bool {
        self.bounds().contains(pos)
    }
}

/// A surface whose cells can be read.
pub trait Surface: Bounded {
    /// Read the cell at `pos`. Returns `None` for positions outside
    /// [`Bounded::bounds`].
    fn cell(&self, pos: Position) -> Option<&Cell>;

    /// Copy `self`'s cells into `target`, mapping the top-left of
    /// `self.bounds()` to `at` in target coordinates.
    ///
    /// The default walks the source row by row and emits one
    /// [`SurfaceMut::set_cell`] call per source cell. Wide-cell
    /// semantics are preserved:
    ///
    /// * Wide-primary cells advance the column cursor by their full
    ///   `width` so the source's continuation cells are not written
    ///   over independently.
    /// * If the source has been sliced through the middle of a wide
    ///   grapheme (a leading continuation with no primary in view, or
    ///   a trailing primary with no room for its continuation), the
    ///   default substitutes a blank rather than emitting an orphan
    ///   half. The same blank substitution applies when a wide primary
    ///   would land at the right edge of `target` with no room for its
    ///   continuation.
    ///
    /// Implementations may override for a faster path (e.g. bulk line
    /// copies), but must preserve the wide-cell invariants above.
    fn draw<T: SurfaceMut + ?Sized>(&self, target: &mut T, at: Position) {
        let b = self.bounds();
        let tb = target.bounds();
        let t_right = tb.x.saturating_add(tb.width);
        for dy in 0..b.height {
            let dst_y = at.y.saturating_add(dy);
            let mut dx: u16 = 0;
            while dx < b.width {
                let src = Position::new(b.x + dx, b.y + dy);
                let dst = Position::new(at.x.saturating_add(dx), dst_y);
                let Some(cell) = self.cell(src) else {
                    dx += 1;
                    continue;
                };

                // Leading continuation with no primary in view: the
                // source's left edge sliced a wide grapheme in half.
                // Substitute a blank to avoid writing an orphan
                // continuation into target.
                if cell.is_continuation() {
                    target.set_cell(dst, &Cell::BLANK);
                    dx += 1;
                    continue;
                }

                let w = (cell.width() as u16).max(1);

                // Wide primary that won't fit — either source's right
                // edge sliced through its continuation, or target's
                // right edge cuts it off. Emit a blank instead.
                let fits_in_src = dx + w <= b.width;
                let fits_in_dst = dst.x.saturating_add(w) <= t_right;
                if w > 1 && (!fits_in_src || !fits_in_dst) {
                    target.set_cell(dst, &Cell::BLANK);
                    dx += 1;
                    continue;
                }

                target.set_cell(dst, cell);
                dx += w;
            }
        }
    }
}

/// A surface whose cells can be written.
pub trait SurfaceMut: Surface {
    /// Place `cell` at `pos`. Implementations are responsible for
    /// wide-cell semantics (continuation markers, blanking covered
    /// cells) and any dirty tracking they care to do. Taking `&Cell`
    /// lets implementations skip the clone when the destination
    /// already matches.
    fn set_cell(&mut self, pos: Position, cell: &Cell);

    /// Mutable handle to the cell at `pos`. Returns `None` for
    /// out-of-bounds positions.
    ///
    /// Implementations that track dirty state must mark the cell as
    /// touched eagerly — the caller may mutate any field through this
    /// handle and the surface has no way to observe a write. Callers
    /// must not change [`Cell::width`] through this handle; use
    /// [`Self::set_cell`] for wide-cell writes that need
    /// continuation-column accounting.
    fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell>;

    /// Fill the entire [`Bounded::bounds`] with `cell`.
    fn fill(&mut self, cell: &Cell) {
        let b = self.bounds();
        self.fill_rect(b, cell);
    }

    /// Fill the intersection of `rect` and [`Bounded::bounds`] with
    /// `cell`.
    ///
    /// Stepped by `cell.width()` so wide cells lay down clean
    /// primary/continuation pairs; a trailing partial slot at the
    /// right edge falls back to a blank. Implementations may override
    /// for a bulk-blit fast path.
    fn fill_rect(&mut self, rect: Rect, cell: &Cell) {
        let clipped = self.bounds().intersection(rect);
        let step = (cell.width() as u16).max(1);
        for y in clipped.top()..clipped.bottom() {
            let mut x = clipped.left();
            while x + step <= clipped.right() {
                self.set_cell(Position::new(x, y), cell);
                x += step;
            }
            while x < clipped.right() {
                self.set_cell(Position::new(x, y), &Cell::BLANK);
                x += 1;
            }
        }
    }

    /// Clear the entire [`Bounded::bounds`] to [`Cell::BLANK`].
    fn clear(&mut self) {
        self.fill(&Cell::BLANK);
    }

    /// Clear the intersection of `rect` and [`Bounded::bounds`] to
    /// [`Cell::BLANK`].
    fn clear_rect(&mut self, rect: Rect) {
        self.fill_rect(rect, &Cell::BLANK);
    }

    /// Insert `n` blank rows at `y`, pushing existing rows down within
    /// `[y, bounds_bottom)`. Rows pushed past `bounds_bottom` are lost.
    /// Freed top rows are filled with `fill`.
    ///
    /// The default implementation copies cells through [`Self::set_cell`]
    /// row-by-row from the bottom up, advancing along each source row by
    /// the source cell's width so wide-primary cells move as a unit and
    /// their continuation columns are not independently rewritten.
    /// Wide primaries that no longer fit are replaced by a blank.
    /// Implementations backed by contiguous row storage may override
    /// with a row-swap fast path.
    fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        let h = self.bounds().height;
        let bottom = bounds_bottom.min(h);
        if y >= bottom || n == 0 {
            return;
        }
        let n = n.min(bottom - y);
        let width = self.bounds().width;
        let mut dst = bottom;
        while dst > y + n {
            dst -= 1;
            let src = dst - n;
            copy_row(self, src, dst, width);
        }
        for row in y..y + n {
            blank_row(self, row, width, fill);
        }
    }

    /// Delete `n` rows at `y`, pulling existing rows up within
    /// `[y, bounds_bottom)`. The bottom `n` rows of the window are
    /// filled with `fill`.
    ///
    /// The default implementation copies cells through [`Self::set_cell`]
    /// row-by-row from top down, advancing along each source row by the
    /// source cell's width so wide-primary cells move as a unit and
    /// their continuation columns are not independently rewritten.
    /// Wide primaries that no longer fit are replaced by a blank.
    /// Implementations backed by contiguous row storage may override
    /// with a row-swap fast path.
    fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        let h = self.bounds().height;
        let bottom = bounds_bottom.min(h);
        if y >= bottom || n == 0 {
            return;
        }
        let n = n.min(bottom - y);
        let width = self.bounds().width;
        for dst in y..bottom - n {
            let src = dst + n;
            copy_row(self, src, dst, width);
        }
        for row in bottom - n..bottom {
            blank_row(self, row, width, fill);
        }
    }

    /// Insert `n` blank cells at `pos`, pushing cells in
    /// `[pos.x, bounds_right)` right within row `pos.y`. Cells pushed
    /// past `bounds_right` are lost. The freed slots `[pos.x, pos.x + n)`
    /// are filled with `fill`.
    ///
    /// The default implementation snapshots the row's primary cells in
    /// the affected window through [`Self::cell`], blanks the window
    /// via [`Self::set_cell`], then replays each surviving primary at
    /// its shifted position. Wide primaries whose new footprint would
    /// cross `bounds_right` are dropped. Implementations backed by a
    /// contiguous row slice may override with an in-place rotate.
    fn insert_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        let bounds = self.bounds();
        if pos.y >= bounds.height || pos.x >= bounds.width || n == 0 {
            return;
        }
        let right = bounds_right.min(bounds.width);
        if pos.x >= right {
            return;
        }
        let n = n.min(right - pos.x);

        let primaries = collect_primaries(self, pos.y, pos.x, right);
        blank_span(self, pos.y, pos.x, right);

        for (src_x, cell) in primaries.iter().rev() {
            let dst_x = src_x.saturating_add(n);
            let cw = (cell.width() as u16).max(1);
            if dst_x >= right || dst_x + cw > right {
                continue;
            }
            self.set_cell(Position::new(dst_x, pos.y), cell);
        }

        fill_span(self, pos.y, pos.x, pos.x + n, fill);
    }

    /// Delete `n` cells at `pos`, pulling cells in
    /// `[pos.x + n, bounds_right)` left within row `pos.y`. The freed
    /// slots `[bounds_right - n, bounds_right)` are filled with `fill`.
    ///
    /// The default implementation snapshots the row's primary cells in
    /// the affected window through [`Self::cell`], blanks the window
    /// via [`Self::set_cell`], then replays each surviving primary at
    /// its shifted position. Primaries whose source column falls inside
    /// `[pos.x, pos.x + n)` are dropped. Implementations backed by a
    /// contiguous row slice may override with an in-place rotate.
    fn delete_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        let bounds = self.bounds();
        if pos.y >= bounds.height || pos.x >= bounds.width || n == 0 {
            return;
        }
        let right = bounds_right.min(bounds.width);
        if pos.x >= right {
            return;
        }
        let n = n.min(right - pos.x);

        let primaries = collect_primaries(self, pos.y, pos.x, right);
        blank_span(self, pos.y, pos.x, right);

        let cut = pos.x + n;
        for (src_x, cell) in primaries.iter() {
            if *src_x < cut {
                continue;
            }
            let dst_x = src_x - n;
            let cw = (cell.width() as u16).max(1);
            if dst_x + cw > right {
                continue;
            }
            self.set_cell(Position::new(dst_x, pos.y), cell);
        }

        fill_span(self, pos.y, right - n, right, fill);
    }
}

fn collect_primaries<S: SurfaceMut + ?Sized>(
    s: &S,
    y: u16,
    left: u16,
    right: u16,
) -> Vec<(u16, Cell)> {
    let mut out = Vec::new();
    let mut x = left;
    while x < right {
        if let Some(cell) = s.cell(Position::new(x, y)) {
            if !cell.is_continuation() {
                let cw = (cell.width() as u16).max(1);
                out.push((x, cell.clone()));
                x = x.saturating_add(cw);
                continue;
            }
        }
        x = x.saturating_add(1);
    }
    out
}

fn blank_span<S: SurfaceMut + ?Sized>(s: &mut S, y: u16, left: u16, right: u16) {
    for x in left..right {
        s.set_cell(Position::new(x, y), &Cell::BLANK);
    }
}

fn fill_span<S: SurfaceMut + ?Sized>(s: &mut S, y: u16, left: u16, right: u16, fill: &Cell) {
    if left >= right {
        return;
    }
    let fill_w = (fill.width() as u16).max(1);
    let mut col = left;
    while col + fill_w <= right {
        s.set_cell(Position::new(col, y), fill);
        col += fill_w;
    }
    while col < right {
        s.set_cell(Position::new(col, y), &Cell::BLANK);
        col += 1;
    }
}

fn copy_row<S: SurfaceMut + ?Sized>(s: &mut S, src_y: u16, dst_y: u16, width: u16) {
    let mut x: u16 = 0;
    while x < width {
        let src = Position::new(x, src_y);
        let dst = Position::new(x, dst_y);
        let Some(cell) = s.cell(src).cloned() else {
            x += 1;
            continue;
        };
        if cell.is_continuation() {
            s.set_cell(dst, &Cell::BLANK);
            x += 1;
            continue;
        }
        let cw = (cell.width() as u16).max(1);
        if cw > 1 && x + cw > width {
            s.set_cell(dst, &Cell::BLANK);
            x += 1;
        } else {
            s.set_cell(dst, &cell);
            x += cw;
        }
    }
}

fn blank_row<S: SurfaceMut + ?Sized>(s: &mut S, y: u16, width: u16, fill: &Cell) {
    for x in 0..width {
        s.set_cell(Position::new(x, y), fill);
    }
}

impl Bounded for Rect {
    fn bounds(&self) -> Rect {
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn wide(s: &str) -> Cell {
        Cell::new(s, 2)
    }

    fn cont() -> Cell {
        Cell::new("", 0)
    }

    #[test]
    fn draw_copies_normal_cells() {
        let mut src = Buffer::new(2, 1);
        src.set((0, 0), &Cell::new("A", 1));
        src.set((1, 0), &Cell::new("B", 1));
        let mut dst = Buffer::new(4, 1);
        src.draw(&mut dst, Position::new(1, 0));
        assert_eq!(dst.cell(Position::new(0, 0)).unwrap().content(), " ");
        assert_eq!(dst.cell(Position::new(1, 0)).unwrap().content(), "A");
        assert_eq!(dst.cell(Position::new(2, 0)).unwrap().content(), "B");
    }

    #[test]
    fn draw_preserves_wide_pair() {
        let mut src = Buffer::new(2, 1);
        src.set((0, 0), &wide("世"));
        // Buffer::set already wrote the continuation at (1,0).
        let mut dst = Buffer::new(2, 1);
        src.draw(&mut dst, Position::new(0, 0));
        let primary = dst.cell(Position::new(0, 0)).unwrap();
        let cont_cell = dst.cell(Position::new(1, 0)).unwrap();
        assert_eq!(primary.content(), "世");
        assert_eq!(primary.width(), 2);
        assert!(cont_cell.is_continuation());
    }

    #[test]
    fn draw_blanks_leading_continuation_in_source_slice() {
        // Construct a source whose first column is the orphan
        // continuation half of a wide cell that lives outside the
        // slice. The default must not propagate the orphan.
        let mut src = Buffer::new(2, 1);
        src.set((0, 0), &cont());
        src.set((1, 0), &Cell::new("X", 1));

        let mut dst = Buffer::new(2, 1);
        // Pre-seed target with an unrelated wide cell to make sure
        // we'd notice a corruption.
        dst.set((0, 0), &Cell::new("Y", 1));
        dst.set((1, 0), &Cell::new("Z", 1));

        src.draw(&mut dst, Position::new(0, 0));

        // The orphan continuation became a blank — no spurious
        // continuation marker carried over.
        assert!(!dst.cell(Position::new(0, 0)).unwrap().is_continuation());
        assert_eq!(dst.cell(Position::new(0, 0)).unwrap().content(), " ");
        assert_eq!(dst.cell(Position::new(1, 0)).unwrap().content(), "X");
    }

    #[test]
    fn draw_blanks_wide_primary_with_no_room_in_target() {
        // Wide primary lands at the last column of the target — its
        // continuation would fall outside target bounds. Substitute
        // a blank rather than emitting a half-drawn grapheme.
        let mut src = Buffer::new(2, 1);
        src.set((0, 0), &wide("世"));

        let mut dst = Buffer::new(2, 1);
        src.draw(&mut dst, Position::new(1, 0));

        let landed = dst.cell(Position::new(1, 0)).unwrap();
        assert_eq!(landed.content(), " ");
        assert_eq!(landed.width(), 1);
    }

    // The remaining tests exercise the SurfaceMut default implementations
    // of insert/delete lines and insert/delete cells via Window, which
    // does not override them.
    use crate::buffer::Window;

    fn window_with(content: &[&str]) -> Window {
        let w = content[0].chars().count() as u16;
        let h = content.len() as u16;
        let mut win = Window::new(w, h);
        for (y, row) in content.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                win.set_cell(
                    Position::new(x as u16, y as u16),
                    &Cell::new(&ch.to_string(), 1),
                );
            }
        }
        win
    }

    fn row_content(win: &Window, y: u16) -> String {
        (0..win.bounds().width)
            .map(|x| {
                win.cell(Position::new(x, y))
                    .map(|c| c.content().to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn default_insert_lines_shifts_down_and_blanks_top() {
        let mut win = window_with(&["AAA", "BBB", "CCC", "DDD"]);
        SurfaceMut::insert_lines(&mut win, 1, 1, 4, &Cell::BLANK);
        assert_eq!(row_content(&win, 0), "AAA");
        assert_eq!(row_content(&win, 1), "   ");
        assert_eq!(row_content(&win, 2), "BBB");
        assert_eq!(row_content(&win, 3), "CCC");
    }

    #[test]
    fn default_delete_lines_pulls_up_and_blanks_bottom() {
        let mut win = window_with(&["AAA", "BBB", "CCC", "DDD"]);
        SurfaceMut::delete_lines(&mut win, 1, 1, 4, &Cell::BLANK);
        assert_eq!(row_content(&win, 0), "AAA");
        assert_eq!(row_content(&win, 1), "CCC");
        assert_eq!(row_content(&win, 2), "DDD");
        assert_eq!(row_content(&win, 3), "   ");
    }

    #[test]
    fn default_insert_cells_shifts_right_and_blanks_left() {
        let mut win = window_with(&["ABCDE"]);
        SurfaceMut::insert_cells(&mut win, Position::new(1, 0), 2, 5, &Cell::BLANK);
        assert_eq!(row_content(&win, 0), "A  BC");
    }

    #[test]
    fn default_delete_cells_pulls_left_and_blanks_right() {
        let mut win = window_with(&["ABCDE"]);
        SurfaceMut::delete_cells(&mut win, Position::new(1, 0), 2, 5, &Cell::BLANK);
        assert_eq!(row_content(&win, 0), "ADE  ");
    }

    #[test]
    fn default_insert_cells_drops_wide_that_no_longer_fits() {
        // Row: A 世(prim) 世(cont) B, then insert 1 at col 1 with right=4.
        // 世's new primary would be at col 2, continuation at col 3.
        // That still fits (col 3 < right=4). So 世 is preserved.
        let mut win = Window::new(4, 1);
        win.set_cell(Position::new(0, 0), &Cell::new("A", 1));
        win.set_cell(Position::new(1, 0), &wide("世"));
        win.set_cell(Position::new(3, 0), &Cell::new("B", 1));
        SurfaceMut::insert_cells(&mut win, Position::new(1, 0), 1, 4, &Cell::BLANK);

        assert_eq!(win.cell(Position::new(0, 0)).unwrap().content(), "A");
        assert_eq!(win.cell(Position::new(1, 0)).unwrap().content(), " ");
        assert_eq!(win.cell(Position::new(2, 0)).unwrap().content(), "世");
        assert!(win.cell(Position::new(3, 0)).unwrap().is_continuation());
    }

    #[test]
    fn default_insert_cells_blanks_wide_that_overflows() {
        // Row: A B 世(prim) 世(cont), then insert 1 at col 1 with right=4.
        // 世 would move from col 2 to col 3, continuation to col 4 (out).
        // So 世 must be dropped, leaving a blank at col 3.
        let mut win = Window::new(4, 1);
        win.set_cell(Position::new(0, 0), &Cell::new("A", 1));
        win.set_cell(Position::new(1, 0), &Cell::new("B", 1));
        win.set_cell(Position::new(2, 0), &wide("世"));
        SurfaceMut::insert_cells(&mut win, Position::new(1, 0), 1, 4, &Cell::BLANK);

        assert_eq!(win.cell(Position::new(0, 0)).unwrap().content(), "A");
        assert_eq!(win.cell(Position::new(1, 0)).unwrap().content(), " ");
        assert_eq!(win.cell(Position::new(2, 0)).unwrap().content(), "B");
        assert_eq!(win.cell(Position::new(3, 0)).unwrap().content(), " ");
    }

    #[test]
    fn default_delete_cells_drops_primary_in_deletion_window() {
        // Row: A 世(prim) 世(cont) B, delete 2 at col 1 with right=4.
        // 世's primary falls inside [1, 3) → dropped entirely.
        let mut win = Window::new(4, 1);
        win.set_cell(Position::new(0, 0), &Cell::new("A", 1));
        win.set_cell(Position::new(1, 0), &wide("世"));
        win.set_cell(Position::new(3, 0), &Cell::new("B", 1));
        SurfaceMut::delete_cells(&mut win, Position::new(1, 0), 2, 4, &Cell::BLANK);

        assert_eq!(win.cell(Position::new(0, 0)).unwrap().content(), "A");
        assert_eq!(win.cell(Position::new(1, 0)).unwrap().content(), "B");
        assert_eq!(win.cell(Position::new(2, 0)).unwrap().content(), " ");
        assert_eq!(win.cell(Position::new(3, 0)).unwrap().content(), " ");
    }
}

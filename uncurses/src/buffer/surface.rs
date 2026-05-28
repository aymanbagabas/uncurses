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
}

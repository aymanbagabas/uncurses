//! Cell-grid surface traits.
//!
//! Buffer-like types ([`Buffer`](super::Buffer), [`Window`](super::Window),
//! [`View`](super::View), [`TextBuffer`](super::TextBuffer), and
//! [`Screen`](crate::screen::Screen)) all expose a rectangular region of
//! [`Cell`]s. This module defines the traits that let drawing code use
//! those regions without depending on a concrete storage type.
//!
//! ## Trait layers
//!
//! | trait          | adds                            | required methods                         |
//! |----------------|---------------------------------|------------------------------------------|
//! | [`Bounded`]    | rectangular extent              | [`bounds`](Bounded::bounds)              |
//! | [`Surface`]    | read-only cell access           | [`arena`](Surface::arena), `cell_ref`    |
//! | [`SurfaceMut`] | writes and terminal operations  | `set_ref`                                |
//!
//! [`Surface`] extends [`Bounded`], and [`SurfaceMut`] extends
//! [`Surface`]. Default methods supply common behavior such as
//! [`Surface::draw`], [`SurfaceMut::fill_rect`],
//! [`SurfaceMut::insert_lines`], and [`SurfaceMut::delete_cells`].
//! Implementations may override defaults for storage-specific fast paths
//! as long as they preserve the same visible semantics.
//!
//! ## Two cell types
//!
//! A surface stores [`Ref`](crate::renderer::packed::Ref): three interned ids, eight
//! bytes, meaningful only against the surface's own
//! [`Arena`](crate::renderer::packed::arena::Arena). The renderer diffs frames by
//! comparing those ids.
//!
//! [`Cell`] is what the public methods take and return. It owns its content
//! and style, so it carries no id provenance and is safe to hold, build, and
//! compare anywhere. [`Surface::cell`] resolves and
//! [`SurfaceMut::set_cell`] interns, both at the surface boundary.
//!
//! The required methods work in `Ref` and are therefore implementable only
//! inside this crate, which is what keeps ids from escaping.
//!
//! ## Coordinates and bounds
//!
//! Coordinates are terminal-cell coordinates in the implementer's own
//! coordinate space. A surface reports the rectangle where reads and writes
//! are meaningful through [`Bounded::bounds`]; attempts outside that region
//! should either return `None` or have no effect, as documented by each
//! method.
//!
//! ```text
//! Rect::new(2, 1, 4, 2)
//!
//! y=1      x=2 x=3 x=4 x=5
//!        ┌───┬───┬───┬───┐
//!        │   │   │   │   │
//! y=2    ├───┼───┼───┼───┤
//!        │   │   │   │   │
//!        └───┴───┴───┴───┘
//! ```
//!
//! ## Wide-cell invariants
//!
//! A wide grapheme occupies a primary cell plus a continuation placeholder
//! in the following column. Methods that copy or fill cells step by the
//! primary cell's display width and substitute blanks when a wide grapheme
//! would be split by a source slice, a destination edge, or a fill region.

use crate::cell::Cell;
use crate::layout::{Position, Rect};

/// A value with a rectangular extent in terminal-cell coordinates.
///
/// `Bounded` is the base trait for readable and writable surfaces. It does
/// not imply that cells can be inspected or modified; it only describes the
/// region where a value exists in its own coordinate space.
///
/// Implement this trait for types that can answer "what rectangle do I
/// cover?" and then add [`Surface`] or [`SurfaceMut`] when cell access is
/// available.
pub trait Bounded {
    /// Return the valid region in this value's own coordinate space.
    ///
    /// # Returns
    ///
    /// A [`Rect`] whose `x`/`y` form the top-left coordinate and whose
    /// `width`/`height` form the extent in terminal cells.
    ///
    /// # Panics
    ///
    /// Implementations should not panic.
    ///
    /// # Usage notes
    ///
    /// For most storage-backed surfaces the origin is `(0, 0)`. Clipping
    /// adaptors may report non-zero origins when they expose a sub-rectangle
    /// in the wrapped surface's coordinate space.
    fn bounds(&self) -> Rect;

    /// Return the width of [`Self::bounds`] in terminal cell columns.
    ///
    /// # Returns
    ///
    /// `self.bounds().width`.
    ///
    /// # Panics
    ///
    /// Never panics unless [`Self::bounds`] panics.
    ///
    /// # Usage notes
    ///
    /// This is a convenience method; override only if computing full bounds
    /// is meaningfully more expensive than returning the width.
    fn width(&self) -> u16 {
        self.bounds().width
    }

    /// Return the height of [`Self::bounds`] in terminal cell rows.
    ///
    /// # Returns
    ///
    /// `self.bounds().height`.
    ///
    /// # Panics
    ///
    /// Never panics unless [`Self::bounds`] panics.
    ///
    /// # Usage notes
    ///
    /// This is a convenience method; override only if computing full bounds
    /// is meaningfully more expensive than returning the height.
    fn height(&self) -> u16 {
        self.bounds().height
    }

    /// Test whether a position lies inside [`Self::bounds`].
    ///
    /// # Parameters
    ///
    /// - `pos`: coordinate in this value's coordinate space.
    ///
    /// # Returns
    ///
    /// `true` when `pos` is inside the half-open rectangle described by
    /// [`Self::bounds`], otherwise `false`.
    ///
    /// # Panics
    ///
    /// Never panics unless [`Self::bounds`] panics.
    ///
    /// # Usage notes
    ///
    /// Surfaces commonly use this to clip reads and writes before touching
    /// their backing storage.
    fn contains(&self, pos: Position) -> bool {
        self.bounds().contains(pos)
    }
}

/// A rectangular cell grid that can be read.
///
/// `Surface` adds immutable cell access to [`Bounded`]. It is the trait to
/// use for render sources, snapshots, and helpers that only need to inspect
/// cells or copy them into another surface.
///
/// The provided [`Surface::draw`] method is intentionally conservative
/// about wide cells: it avoids producing orphan continuation columns and
/// blanks wide primaries that would be split by source or destination
/// bounds.
pub trait Surface: Bounded {
    /// Read the cell at a position.
    ///
    /// # Parameters
    ///
    /// - `pos`: coordinate in this surface's coordinate space.
    ///
    /// # Returns
    ///
    /// `Some(Cell)` when `pos` is readable, or `None` when it is outside
    /// [`Bounded::bounds`] or otherwise unavailable.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// A returned cell may be a wide primary or a continuation placeholder.
    /// Callers that walk rows should advance by `cell.width().max(1)` for
    /// primary cells and handle continuations explicitly.
    ///
    /// A surface that stores cells in a packed form resolves them here, so
    /// this is a read API rather than a scanning one.
    fn cell(&self, pos: Position) -> Option<Cell>;

    /// Copy `self`'s cells into `target`, mapping the top-left of
    /// `self.bounds()` to `at` in target coordinates.
    ///
    /// # Parameters
    ///
    /// - `target`: writable destination surface.
    /// - `at`: destination coordinate for the source bounds' top-left
    ///   corner.
    ///
    /// # Behavior
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
    ///
    /// # Panics
    ///
    /// The default implementation does not panic unless the destination's
    /// [`SurfaceMut::set_cell`] implementation panics.
    ///
    /// # Usage notes
    ///
    /// Destination clipping is delegated to `target`. Cells whose
    /// destination coordinate lies outside the target bounds are passed to
    /// `set_cell`, which is expected to ignore them.
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
                    target.set_cell(dst, &Cell::default());
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
                    target.set_cell(dst, &Cell::default());
                    dx += 1;
                    continue;
                }

                target.set_cell(dst, &cell);
                dx += w;
            }
        }
    }
}

/// A rectangular cell grid that can be modified.
///
/// `SurfaceMut` is the trait to use for render targets. It includes a
/// primitive cell write plus higher-level operations used by terminal
/// emulation and drawing code: fills, clears, line insertion/deletion, and
/// cell insertion/deletion within a row.
///
/// Implementations are responsible for preserving the same wide-cell
/// invariants as [`Buffer`](super::Buffer): a wide primary owns the
/// following continuation slot, continuations should not become visible
/// without their primary, and clipped wide writes should leave blanks
/// rather than half a grapheme.
pub trait SurfaceMut: Surface {
    /// Place `cell` at `pos`.
    ///
    /// # Parameters
    ///
    /// - `pos`: destination coordinate in this surface's coordinate space.
    /// - `cell`: cell to write.
    ///
    /// # Panics
    ///
    /// Never panics. Out-of-bounds writes are ignored.
    ///
    /// # Usage notes
    ///
    /// Implementations are responsible for wide-cell semantics
    /// (continuation markers, blanking covered cells) and any dirty tracking
    /// they care to do. Out-of-bounds writes are ignored.
    fn set_cell(&mut self, pos: Position, cell: &Cell);

    /// Fill the entire surface bounds with `cell`.
    ///
    /// # Parameters
    ///
    /// - `cell`: fill cell to write repeatedly.
    ///
    /// # Returns
    ///
    /// Nothing.
    ///
    /// # Panics
    ///
    /// Does not panic unless [`Self::fill_rect`] panics.
    ///
    /// # Usage notes
    ///
    /// This delegates to [`Self::fill_rect`] with [`Bounded::bounds`].
    /// Wide fills are handled by `fill_rect`.
    fn fill(&mut self, cell: &Cell) {
        let b = self.bounds();
        self.fill_rect(b, cell);
    }

    /// Fill the intersection of `rect` and [`Bounded::bounds`] with
    /// `cell`.
    ///
    /// # Parameters
    ///
    /// - `rect`: requested fill rectangle in this surface's coordinate
    ///   space.
    /// - `cell`: fill cell to write.
    ///
    /// # Behavior
    ///
    /// Stepped by `cell.width()` so wide cells lay down clean
    /// primary/continuation pairs; a trailing partial slot at the
    /// right edge falls back to a blank. Implementations may override
    /// for a bulk-blit fast path.
    ///
    /// # Panics
    ///
    /// The default implementation does not panic unless [`Self::set_cell`]
    /// panics.
    ///
    /// # Usage notes
    ///
    /// Empty intersections are no-ops. A wide fill into an odd-width region
    /// leaves the final single column blank because a two-column grapheme
    /// cannot fit there.
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
                self.set_cell(Position::new(x, y), &Cell::default());
                x += 1;
            }
        }
    }

    /// Clear the entire surface bounds to [`Cell::default`].
    ///
    /// # Returns
    ///
    /// Nothing.
    ///
    /// # Panics
    ///
    /// Does not panic unless [`Self::fill`] panics.
    ///
    /// # Usage notes
    ///
    /// This is equivalent to `self.fill(&Cell::BLANK)`.
    fn clear(&mut self) {
        self.fill(&Cell::default());
    }

    /// Clear a rectangle to [`Cell::default`].
    ///
    /// # Parameters
    ///
    /// - `rect`: requested clear rectangle in this surface's coordinate
    ///   space.
    ///
    /// # Returns
    ///
    /// Nothing.
    ///
    /// # Panics
    ///
    /// Does not panic unless [`Self::fill_rect`] panics.
    ///
    /// # Usage notes
    ///
    /// Only the intersection of `rect` and [`Bounded::bounds`] is modified.
    fn clear_rect(&mut self, rect: Rect) {
        self.fill_rect(rect, &Cell::default());
    }

    /// Insert `n` blank rows at `y`, pushing existing rows down within
    /// `[y, bounds_bottom)`. Rows pushed past `bounds_bottom` are lost.
    /// Freed top rows are filled with `fill`.
    ///
    /// # Parameters
    ///
    /// - `y`: first row to insert at, in this surface's coordinate space.
    /// - `n`: number of rows to insert.
    /// - `bounds_bottom`: exclusive lower row bound for the affected region.
    /// - `fill`: cell used to fill the newly opened rows.
    ///
    /// # Behavior
    ///
    /// The default implementation copies cells through [`Self::set_cell`]
    /// row-by-row from the bottom up, advancing along each source row by
    /// the source cell's width so wide-primary cells move as a unit and
    /// their continuation columns are not independently rewritten.
    /// Wide primaries that no longer fit are replaced by a blank.
    /// Implementations backed by contiguous row storage may override
    /// with a row-swap fast path.
    ///
    /// # Panics
    ///
    /// The default implementation does not panic unless [`Self::set_cell`]
    /// panics.
    ///
    /// # Usage notes
    ///
    /// `bounds_bottom` is clamped to the surface height. Calls with
    /// `n == 0` or `y >= bounds_bottom` are no-ops.
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
    /// # Parameters
    ///
    /// - `y`: first row to delete, in this surface's coordinate space.
    /// - `n`: number of rows to delete.
    /// - `bounds_bottom`: exclusive lower row bound for the affected region.
    /// - `fill`: cell used to fill the freed bottom rows.
    ///
    /// # Behavior
    ///
    /// The default implementation copies cells through [`Self::set_cell`]
    /// row-by-row from top down, advancing along each source row by the
    /// source cell's width so wide-primary cells move as a unit and
    /// their continuation columns are not independently rewritten.
    /// Wide primaries that no longer fit are replaced by a blank.
    /// Implementations backed by contiguous row storage may override
    /// with a row-swap fast path.
    ///
    /// # Panics
    ///
    /// The default implementation does not panic unless [`Self::set_cell`]
    /// panics.
    ///
    /// # Usage notes
    ///
    /// `bounds_bottom` is clamped to the surface height. Calls with
    /// `n == 0` or `y >= bounds_bottom` are no-ops.
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
    /// # Parameters
    ///
    /// - `pos`: row and starting column for insertion.
    /// - `n`: number of cells to insert.
    /// - `bounds_right`: exclusive right column bound for the affected row
    ///   region.
    /// - `fill`: cell used to fill the newly opened slots.
    ///
    /// # Behavior
    ///
    /// The default implementation snapshots the row's primary cells in
    /// the affected window through [`Surface::cell`], blanks the window
    /// via [`Self::set_cell`], then replays each surviving primary at
    /// its shifted position. Wide primaries whose new footprint would
    /// cross `bounds_right` are dropped. Implementations backed by a
    /// contiguous row slice may override with an in-place rotate.
    ///
    /// # Panics
    ///
    /// The default implementation does not panic unless [`Self::set_cell`]
    /// panics.
    ///
    /// # Usage notes
    ///
    /// `bounds_right` is clamped to the surface width. Calls with `n == 0`,
    /// an out-of-bounds row, or `pos.x >= bounds_right` are no-ops.
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
    /// # Parameters
    ///
    /// - `pos`: row and starting column for deletion.
    /// - `n`: number of cells to delete.
    /// - `bounds_right`: exclusive right column bound for the affected row
    ///   region.
    /// - `fill`: cell used to fill the freed right-edge slots.
    ///
    /// # Behavior
    ///
    /// The default implementation snapshots the row's primary cells in
    /// the affected window through [`Surface::cell`], blanks the window
    /// via [`Self::set_cell`], then replays each surviving primary at
    /// its shifted position. Primaries whose source column falls inside
    /// `[pos.x, pos.x + n)` are dropped. Implementations backed by a
    /// contiguous row slice may override with an in-place rotate.
    ///
    /// # Panics
    ///
    /// The default implementation does not panic unless [`Self::set_cell`]
    /// panics.
    ///
    /// # Usage notes
    ///
    /// `bounds_right` is clamped to the surface width. Calls with `n == 0`,
    /// an out-of-bounds row, or `pos.x >= bounds_right` are no-ops.
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
        if let Some(cell) = s.cell(Position::new(x, y))
            && !cell.is_continuation()
        {
            let cw = (cell.width() as u16).max(1);
            out.push((x, cell));
            x = x.saturating_add(cw);
            continue;
        }
        x = x.saturating_add(1);
    }
    out
}

fn blank_span<S: SurfaceMut + ?Sized>(s: &mut S, y: u16, left: u16, right: u16) {
    for x in left..right {
        s.set_cell(Position::new(x, y), &Cell::default());
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
        s.set_cell(Position::new(col, y), &Cell::default());
        col += 1;
    }
}

fn copy_row<S: SurfaceMut + ?Sized>(s: &mut S, src_y: u16, dst_y: u16, width: u16) {
    let mut x: u16 = 0;
    while x < width {
        let src = Position::new(x, src_y);
        let dst = Position::new(x, dst_y);
        let Some(cell) = s.cell(src) else {
            x += 1;
            continue;
        };
        if cell.is_continuation() {
            s.set_cell(dst, &Cell::default());
            x += 1;
            continue;
        }
        let cw = (cell.width() as u16).max(1);
        if cw > 1 && x + cw > width {
            s.set_cell(dst, &Cell::default());
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
        Cell::from(s)
    }

    fn cont() -> Cell {
        Cell::continuation()
    }

    #[test]
    fn draw_copies_normal_cells() {
        let mut src = Buffer::new(2, 1);
        src.set_cell(Position::new(0, 0), &Cell::from('A'));
        src.set_cell(Position::new(1, 0), &Cell::from('B'));
        let mut dst = Buffer::new(4, 1);
        src.draw(&mut dst, Position::new(1, 0));
        assert_eq!(
            dst.cell(Position::new(0, 0)).unwrap().content.char(),
            Some(' ')
        );
        assert_eq!(
            dst.cell(Position::new(1, 0)).unwrap().content.char(),
            Some('A')
        );
        assert_eq!(
            dst.cell(Position::new(2, 0)).unwrap().content.char(),
            Some('B')
        );
    }

    #[test]
    fn draw_preserves_wide_pair() {
        let mut src = Buffer::new(2, 1);
        src.set_cell(Position::new(0, 0), &wide("世"));
        // Buffer::set already wrote the continuation at (1,0).
        let mut dst = Buffer::new(2, 1);
        src.draw(&mut dst, Position::new(0, 0));
        let primary = dst.cell(Position::new(0, 0)).unwrap();
        let cont_cell = dst.cell(Position::new(1, 0)).unwrap();
        assert_eq!(primary.content.char(), Some('世'));
        assert_eq!(primary.width(), 2);
        assert!(cont_cell.is_continuation());
    }

    #[test]
    fn draw_blanks_leading_continuation_in_source_slice() {
        // Construct a source whose first column is the orphan
        // continuation half of a wide cell that lives outside the
        // slice. The default must not propagate the orphan.
        let mut src = Buffer::new(2, 1);
        src.set_cell(Position::new(0, 0), &cont());
        src.set_cell(Position::new(1, 0), &Cell::from('X'));

        let mut dst = Buffer::new(2, 1);
        // Pre-seed target with an unrelated wide cell to make sure
        // we'd notice a corruption.
        dst.set_cell(Position::new(0, 0), &Cell::from('Y'));
        dst.set_cell(Position::new(1, 0), &Cell::from('Z'));

        src.draw(&mut dst, Position::new(0, 0));

        // The orphan continuation became a blank — no spurious
        // continuation marker carried over.
        assert!(!dst.cell(Position::new(0, 0)).unwrap().is_continuation());
        assert_eq!(
            dst.cell(Position::new(0, 0)).unwrap().content.char(),
            Some(' ')
        );
        assert_eq!(
            dst.cell(Position::new(1, 0)).unwrap().content.char(),
            Some('X')
        );
    }

    #[test]
    fn draw_blanks_wide_primary_with_no_room_in_target() {
        // Wide primary lands at the last column of the target — its
        // continuation would fall outside target bounds. Substitute
        // a blank rather than emitting a half-drawn grapheme.
        let mut src = Buffer::new(2, 1);
        src.set_cell(Position::new(0, 0), &wide("世"));

        let mut dst = Buffer::new(2, 1);
        src.draw(&mut dst, Position::new(1, 0));

        let landed = dst.cell(Position::new(1, 0)).unwrap();
        assert_eq!(landed.content.char(), Some(' '));
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
                win.set_cell(Position::new(x as u16, y as u16), &Cell::from(ch));
            }
        }
        win
    }

    fn row_content(win: &Window, y: u16) -> String {
        (0..win.bounds().width)
            .map(|x| {
                win.cell(Position::new(x, y))
                    .map(|c| c.to_string().to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn default_insert_lines_shifts_down_and_blanks_top() {
        let mut win = window_with(&["AAA", "BBB", "CCC", "DDD"]);
        SurfaceMut::insert_lines(&mut win, 1, 1, 4, &Cell::default());
        assert_eq!(row_content(&win, 0), "AAA");
        assert_eq!(row_content(&win, 1), "   ");
        assert_eq!(row_content(&win, 2), "BBB");
        assert_eq!(row_content(&win, 3), "CCC");
    }

    #[test]
    fn default_delete_lines_pulls_up_and_blanks_bottom() {
        let mut win = window_with(&["AAA", "BBB", "CCC", "DDD"]);
        SurfaceMut::delete_lines(&mut win, 1, 1, 4, &Cell::default());
        assert_eq!(row_content(&win, 0), "AAA");
        assert_eq!(row_content(&win, 1), "CCC");
        assert_eq!(row_content(&win, 2), "DDD");
        assert_eq!(row_content(&win, 3), "   ");
    }

    #[test]
    fn default_insert_cells_shifts_right_and_blanks_left() {
        let mut win = window_with(&["ABCDE"]);
        SurfaceMut::insert_cells(&mut win, Position::new(1, 0), 2, 5, &Cell::default());
        assert_eq!(row_content(&win, 0), "A  BC");
    }

    #[test]
    fn default_delete_cells_pulls_left_and_blanks_right() {
        let mut win = window_with(&["ABCDE"]);
        SurfaceMut::delete_cells(&mut win, Position::new(1, 0), 2, 5, &Cell::default());
        assert_eq!(row_content(&win, 0), "ADE  ");
    }

    #[test]
    fn default_insert_cells_drops_wide_that_no_longer_fits() {
        // Row: A 世(prim) 世(cont) B, then insert 1 at col 1 with right=4.
        // 世's new primary would be at col 2, continuation at col 3.
        // That still fits (col 3 < right=4). So 世 is preserved.
        let mut win = Window::new(4, 1);
        win.set_cell(Position::new(0, 0), &Cell::from('A'));
        win.set_cell(Position::new(1, 0), &wide("世"));
        win.set_cell(Position::new(3, 0), &Cell::from('B'));
        SurfaceMut::insert_cells(&mut win, Position::new(1, 0), 1, 4, &Cell::default());

        assert_eq!(
            win.cell(Position::new(0, 0)).unwrap().content.char(),
            Some('A')
        );
        assert_eq!(
            win.cell(Position::new(1, 0)).unwrap().content.char(),
            Some(' ')
        );
        assert_eq!(
            win.cell(Position::new(2, 0)).unwrap().content.char(),
            Some('世')
        );
        assert!(win.cell(Position::new(3, 0)).unwrap().is_continuation());
    }

    #[test]
    fn default_insert_cells_blanks_wide_that_overflows() {
        // Row: A B 世(prim) 世(cont), then insert 1 at col 1 with right=4.
        // 世 would move from col 2 to col 3, continuation to col 4 (out).
        // So 世 must be dropped, leaving a blank at col 3.
        let mut win = Window::new(4, 1);
        win.set_cell(Position::new(0, 0), &Cell::from('A'));
        win.set_cell(Position::new(1, 0), &Cell::from('B'));
        win.set_cell(Position::new(2, 0), &wide("世"));
        SurfaceMut::insert_cells(&mut win, Position::new(1, 0), 1, 4, &Cell::default());

        assert_eq!(
            win.cell(Position::new(0, 0)).unwrap().content.char(),
            Some('A')
        );
        assert_eq!(
            win.cell(Position::new(1, 0)).unwrap().content.char(),
            Some(' ')
        );
        assert_eq!(
            win.cell(Position::new(2, 0)).unwrap().content.char(),
            Some('B')
        );
        assert_eq!(
            win.cell(Position::new(3, 0)).unwrap().content.char(),
            Some(' ')
        );
    }

    #[test]
    fn default_delete_cells_drops_primary_in_deletion_window() {
        // Row: A 世(prim) 世(cont) B, delete 2 at col 1 with right=4.
        // 世's primary falls inside [1, 3) → dropped entirely.
        let mut win = Window::new(4, 1);
        win.set_cell(Position::new(0, 0), &Cell::from('A'));
        win.set_cell(Position::new(1, 0), &wide("世"));
        win.set_cell(Position::new(3, 0), &Cell::from('B'));
        SurfaceMut::delete_cells(&mut win, Position::new(1, 0), 2, 4, &Cell::default());

        assert_eq!(
            win.cell(Position::new(0, 0)).unwrap().content.char(),
            Some('A')
        );
        assert_eq!(
            win.cell(Position::new(1, 0)).unwrap().content.char(),
            Some('B')
        );
        assert_eq!(
            win.cell(Position::new(2, 0)).unwrap().content.char(),
            Some(' ')
        );
        assert_eq!(
            win.cell(Position::new(3, 0)).unwrap().content.char(),
            Some(' ')
        );
    }
}

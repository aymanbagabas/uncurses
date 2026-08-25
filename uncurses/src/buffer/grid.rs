//! Row-major cell storage and the wide-cell invariants that go with it.
//!
//! [`Grid`] is the shared implementation under both
//! [`Buffer`](crate::buffer::Buffer), which stores fat
//! [`Cell`](crate::cell::Cell) values, and the renderer's packed grid, which
//! stores interned ids. Neither the storage layout nor the wide-cell
//! bookkeeping depends on which of the two a cell is, so both live here once
//! rather than twice.
//!
//! ## Wide cells
//!
//! A two-column grapheme occupies a primary slot plus a continuation slot to
//! its right. Every write has to keep that pairing intact: overwriting either
//! half has to clear the other, and a wide cell that no longer fits becomes a
//! blank rather than half a grapheme. [`Grid::set`] is the one place that
//! reasoning lives.

use crate::layout::{Position, Rect};

/// What the wide-cell bookkeeping needs to know about a cell.
///
/// Implemented by both the fat [`Cell`](crate::cell::Cell) and the renderer's
/// packed id form, which is what lets [`Grid`] serve both.
pub(crate) trait GridCell: Clone + PartialEq {
    /// Column footprint: `1` narrow, `2` wide, `0` continuation.
    fn width(&self) -> u8;

    /// Whether this is the primary of a two-column grapheme.
    fn is_wide(&self) -> bool;

    /// Whether this is the right-hand placeholder of a wide primary.
    fn is_continuation(&self) -> bool;

    /// A blank cell in the default style.
    fn blank() -> Self;

    /// The right-hand placeholder for a wide primary.
    fn continuation() -> Self;

    /// A blank that keeps this cell's style.
    ///
    /// Clearing a wide cell has to leave its background behind, or erasing a
    /// wide glyph punches a hole in a coloured run.
    fn blank_like(&self) -> Self;

    /// A continuation that keeps this cell's style.
    ///
    /// A wide primary's two columns share one background, so its
    /// placeholder carries the same style.
    fn continuation_like(&self) -> Self;
}

/// A `width * height` grid of cells in row-major order.
///
/// Column `x` and row `y` map to `cells[y * width + x]`, so each row is a
/// contiguous slice and the whole grid is one allocation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Grid<T> {
    cells: Vec<T>,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

impl<T: GridCell> Grid<T> {
    /// A blank grid of the given size.
    pub(crate) fn new(width: u16, height: u16) -> Self {
        Self {
            cells: vec![T::blank(); (width as usize) * (height as usize)],
            width,
            height,
        }
    }

    /// Column count.
    #[inline]
    pub(crate) fn width(&self) -> u16 {
        self.width
    }

    /// Row count.
    #[inline]
    pub(crate) fn height(&self) -> u16 {
        self.height
    }

    /// The grid's extent, for [`Bounded`](crate::buffer::Bounded).
    #[inline]
    pub(crate) fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
    /// Read one cell, or `None` when `pos` is outside the grid.
    #[inline]
    pub(crate) fn get(&self, pos: Position) -> Option<&T> {
        if pos.y >= self.height || pos.x >= self.width {
            return None;
        }
        let w = self.width as usize;
        Some(&self.cells[(pos.y as usize) * w + (pos.x as usize)])
    }

    /// Borrow one row as a contiguous slice.
    ///
    /// # Parameters
    ///
    /// - `y`: zero-based row index in buffer coordinates.
    ///
    /// # Returns
    ///
    /// `Some(&[Cell])` with length [`Grid::width`] when `y` is inside the
    /// buffer, or `None` when `y >= height`.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// The returned slice may contain continuation cells that belong to
    /// wide primaries earlier in the same row. Do not assume every slice
    /// element starts an independent grapheme.
    #[inline]
    pub(crate) fn line(&self, y: u16) -> Option<&[T]> {
        if y >= self.height {
            return None;
        }
        let w = self.width as usize;
        let start = (y as usize) * w;
        Some(&self.cells[start..start + w])
    }

    /// Mutably borrow one row as a contiguous slice.
    ///
    /// # Parameters
    ///
    /// - `y`: zero-based row index in buffer coordinates.
    ///
    /// # Returns
    ///
    /// `Some(&mut [T])` with length [`Grid::width`] when `y` is inside
    /// the buffer, or `None` when `y >= height`.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Direct slice mutation bypasses the wide-cell accounting performed by
    /// `set`. It is appropriate for whole-row helpers that manage
    /// continuations themselves; prefer `set` for ordinary writes.
    #[inline]
    pub(crate) fn line_mut(&mut self, y: u16) -> Option<&mut [T]> {
        if y >= self.height {
            return None;
        }
        let w = self.width as usize;
        let start = (y as usize) * w;
        Some(&mut self.cells[start..start + w])
    }

    /// Swap the contents of rows `a` and `b` in place.
    pub(crate) fn swap_rows(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        let w = self.width as usize;
        if w == 0 {
            return;
        }
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let split = hi * w;
        let (left, right) = self.cells.split_at_mut(split);
        left[lo * w..(lo + 1) * w].swap_with_slice(&mut right[..w]);
    }

    /// Set a cell at a position while preserving wide-cell invariants.
    ///
    /// # Parameters
    ///
    /// - `pos`: zero-based destination coordinate. Any type convertible into
    ///   [`Position`] is accepted.
    /// - `cell`: cell to clone into the destination.
    ///
    /// # Behavior
    ///
    /// In-bounds narrow writes replace exactly one column, blanking any stale
    /// wide-cell halves they overwrite. In-bounds wide writes place `cell` at
    /// `pos` and write continuation placeholders into the columns it covers.
    /// If a wide cell would extend past the right edge of the row, the
    /// destination column is set to [`Cell::default`] instead.
    ///
    /// Out-of-bounds writes are ignored.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// This is the implementation behind `set` for
    /// `Buffer`. Use it instead of [`Buffer::cell_mut`] whenever the write
    /// might affect the width or role of neighboring cells.
    pub(crate) fn set(&mut self, pos: impl Into<Position>, cell: &T) {
        let pos = pos.into();
        let y = pos.y as usize;
        let x = pos.x as usize;
        let width = self.width as usize;

        if pos.y >= self.height || x >= width {
            return;
        }

        let line = &mut self.cells[y * width..(y + 1) * width];

        // If we're overwriting a wide cell's continuation, blank the primary
        // cell that owns it — but only when the *incoming* cell is not itself
        // a continuation. A continuation-to-continuation write is a no-op on
        // the cell's identity (e.g. it happens when a render buffer mirrors
        // a model buffer cell-by-cell after the primary wide cell has just
        // been written one column to the left) and must not destroy the
        // wide cell we are in the middle of mirroring.
        if x > 0 && line[x].is_continuation() && !cell.is_continuation() {
            // Walk backward to find the primary cell
            let mut pc = x - 1;
            while pc > 0 && line[pc].is_continuation() {
                pc -= 1;
            }
            // Verify we found a wide primary; only Wide cells own
            // the continuation slot to their right.
            if line[pc].is_wide() {
                let pw = line[pc].width() as usize;
                let end = (pc + pw).min(width);
                // Preserve the broken wide cell's bg/attributes on the
                // blanks so clearing it doesn't punch a hole in a colored
                // background.
                let blank = line[pc].blank_like();
                line[pc..end].fill(blank);
            } else if line[pc].is_continuation() {
                // Buffer is corrupted — just blank the single cell
                line[x] = T::blank();
            }
        }

        // If we're overwriting the primary cell of a wide char, blank continuations
        if line[x].is_wide() {
            let w = line[x].width() as usize;
            let end = (x + w).min(width);
            let blank = line[x].blank_like();
            line[x + 1..end].fill(blank);
        }

        let cell_width = cell.width() as usize;

        // If the new cell is wide, blank cells it will cover
        if cell.is_wide() {
            for i in x + 1..x + cell_width {
                if i < width {
                    // If we'd overwrite a wide cell's primary, blank its continuations
                    if line[i].is_wide() {
                        let w = line[i].width() as usize;
                        let end = (i + w).min(width);
                        let blank = line[i].blank_like();
                        line[i + 1..end].fill(blank);
                    }
                    // Continuations inherit the wide primary's style so the
                    // cell's bg/attributes are coherent across both columns.
                    line[i] = cell.continuation_like();
                }
            }

            // Truncate at end of line: if wide cell doesn't fit, replace with
            // a blank that keeps the wide cell's bg/attributes.
            if x + cell_width > width {
                line[x] = cell.blank_like();
                return;
            }
        }

        line[x] = cell.clone();
    }

    /// Fill the clipped intersection of `rect` with `cell`. For
    /// width-1 fills this collapses the trait default's per-cell
    /// `set` loop into one `slice::fill` per row, with explicit
    /// wide-cell edge-straddle cleanup at the left and right boundaries
    /// so any wide cell crossing the fill region leaves no orphan
    /// primary or continuation behind. Wide fills (`cell.width() > 1`)
    /// stay on the stepped `set` path so primary/continuation
    /// pairing and the trailing-partial-slot blank are placed by the
    /// same wide-cell handling that `set` already implements.
    pub(crate) fn fill_rect(&mut self, rect: Rect, cell: &T) {
        let clipped = self.bounds().intersection(rect);
        if clipped.is_empty() {
            return;
        }

        let step = (cell.width() as u16).max(1);
        if step > 1 {
            // Stepped wide-cell fill: identical to the trait default.
            // Inlined here so the SurfaceMut::fill_rect dispatch goes
            // through this impl in both arms.
            for y in clipped.top()..clipped.bottom() {
                let mut x = clipped.left();
                while x + step <= clipped.right() {
                    self.set(Position::new(x, y), cell);
                    x += step;
                }
                while x < clipped.right() {
                    self.set(Position::new(x, y), &T::blank());
                    x += 1;
                }
            }
            return;
        }

        let lo = clipped.left() as usize;
        let hi = clipped.right() as usize;
        let row_width = self.width as usize;

        for y in clipped.top()..clipped.bottom() {
            let row_start = (y as usize) * row_width;
            let line = &mut self.cells[row_start..row_start + row_width];

            // Left-edge straddle: a wide cell whose primary sits
            // before `lo` and whose continuation falls at `lo`. Walk
            // back to the primary and blank it (and any siblings
            // outside the fill region) before the bulk fill so the
            // primary isn't left dangling once its continuations get
            // overwritten.
            if lo > 0 && line[lo].is_continuation() {
                let mut p = lo;
                while p > 0 && line[p].is_continuation() {
                    p -= 1;
                }
                if !line[p].is_continuation() {
                    let pw = line[p].width() as usize;
                    let end = (p + pw).min(lo);
                    for slot in &mut line[p..end] {
                        *slot = T::blank();
                    }
                }
            }

            // Right-edge straddle: a wide cell whose primary sits at
            // some `p` in `[lo, hi)` and whose continuations extend
            // past `hi - 1`. The primary itself will be overwritten by
            // the bulk fill, but the orphan continuations at
            // `[hi, p + pw)` need explicit blanking.
            if hi < row_width && line[hi].is_continuation() {
                let mut p = hi - 1;
                while p > lo && line[p].is_continuation() {
                    p -= 1;
                }
                if !line[p].is_continuation() && p + (line[p].width() as usize) > hi {
                    let pw = line[p].width() as usize;
                    let end = (p + pw).min(row_width);
                    for slot in &mut line[hi..end] {
                        *slot = T::blank();
                    }
                }
            }

            line[lo..hi].fill(cell.clone());
        }
    }

    /// Resize the buffer, preserving the top-left intersection.
    ///
    /// # Parameters
    ///
    /// - `width`: new row width in terminal cell columns.
    /// - `height`: new height in terminal cell rows.
    ///
    /// # Behavior
    ///
    /// Cells inside the intersection of the old and new bounds keep their
    /// row and column. Newly exposed cells are filled with [`Cell::default`],
    /// and cells outside the new bounds are discarded.
    ///
    /// # Panics
    ///
    /// Panics if the resized backing store cannot be allocated.
    ///
    /// # Usage notes
    ///
    /// Resizing copies cells structurally and does not reflow wide cells. If
    /// the new right edge cuts through a wide grapheme, the copied
    /// continuation or primary remains as stored until later writes or draws
    /// normalize the affected edge.
    pub(crate) fn resize(&mut self, width: u16, height: u16) {
        if width == self.width && height == self.height {
            return;
        }

        let new_total = (width as usize) * (height as usize);
        let mut new_cells = vec![T::blank(); new_total];

        let copy_w = width.min(self.width) as usize;
        let copy_h = height.min(self.height) as usize;
        let old_w = self.width as usize;
        let new_w = width as usize;
        for y in 0..copy_h {
            let src = y * old_w;
            let dst = y * new_w;
            new_cells[dst..dst + copy_w].clone_from_slice(&self.cells[src..src + copy_w]);
        }

        self.cells = new_cells;
        self.width = width;
        self.height = height;
    }
}

/// Fill an existing row slot in place with `fill`. Wide fills
/// (`fill.width() > 1`) lay down primary + continuation pairs; any
/// trailing slot too narrow to fit another pair is a plain blank.
pub(crate) fn fill_line_into<T: GridCell>(slot: &mut [T], fill: &T) {
    let width = slot.len();
    if fill.width() <= 1 {
        slot.fill(fill.clone());
        return;
    }
    let step = fill.width() as usize;
    let mut x = 0;
    while x + step <= width {
        slot[x] = fill.clone();
        for i in 1..step {
            slot[x + i] = T::continuation();
        }
        x += step;
    }
    while x < width {
        slot[x] = T::blank();
        x += 1;
    }
}

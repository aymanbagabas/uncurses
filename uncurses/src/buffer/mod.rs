//! Cell-grid storage and surface traits.
//!
//! This module provides [`Buffer`], row helpers, rectangular views, and
//! owned [`Window`]s for working with terminal cells. Use it when code
//! needs an off-screen grid, a clipped view of an existing grid, or a
//! common [`Surface`] / [`SurfaceMut`] API that drawing code can target.
//!
//! ## Surface trait family
//!
//! The surface traits describe terminal-cell grids in layers:
//!
//! - [`Bounded`] exposes the rectangular extent of a grid.
//! - [`Surface`] adds read-only cell access and the default
//!   [`draw`](Surface::draw) blit operation.
//! - [`SurfaceMut`] adds mutation, clearing, rectangular fills, and
//!   terminal-style insert/delete operations.
//!
//! Code written against [`Surface`] or [`SurfaceMut`] can operate on a
//! [`Buffer`], [`TextBuffer`], [`Window`], [`View`], or
//! [`Screen`](crate::screen::Screen) without caring where the cells are
//! stored. The shared contract is a rectangular coordinate space of
//! [`Cell`] values addressed by
//! [`Position`].
//!
//! ```rust,ignore
//! use uncurses::buffer::{Buffer, Surface, SurfaceMut};
//! use uncurses::cell::Cell;
//! use uncurses::layout::Position;
//!
//! let mut buf = Buffer::new(4, 2);
//! buf.set_cell(Position::new(0, 0), &Cell::narrow("x"));
//! assert_eq!(buf.cell(Position::new(0, 0)).unwrap().content(), "x");
//! ```
//!
//! ## Buffer storage
//!
//! [`Buffer`] stores a fixed-size grid in one row-major `Vec<Cell>`.
//! Column `x` and row `y` map to `cells[y * width + x]`, so each row is a
//! contiguous slice and cloning the buffer requires only one allocation.
//! Resizing allocates a new row-major backing store, copies the
//! intersection of the old and new extents, and fills new slots with
//! [`Cell::BLANK`](crate::cell::Cell::BLANK).
//!
//! ```text
//!          col:  0   1   2   3
//!              ┌───┬───┬───────┬───┐
//! row 0 (y=0)  │ H │ i │ 日    │ ! │
//!              └───┴───┴───────┴───┘
//!                         ▲
//!                         └─ wide primary at x=2 covers columns 2 and 3
//! ```
//!
//! ## Wide cells and drawing
//!
//! A two-column grapheme is represented by a wide primary cell followed by
//! a continuation placeholder in the next column. [`Buffer::set`] writes
//! that placeholder automatically, blanks stale halves when overwriting an
//! existing wide cell, and replaces a wide cell with a blank when it would
//! not fit at the end of a row. The default [`Surface::draw`] implementation
//! preserves the same invariant when blitting between surfaces: orphan
//! continuations and clipped wide primaries are emitted as blanks.

pub mod ops;
pub mod surface;
pub mod view;
pub mod window;

mod line;
#[cfg(test)]
mod tests;
mod text_buffer;

pub use line::{Line, blank_line, fill_line, fill_line_into};
pub use surface::{Bounded, Surface, SurfaceMut};
pub use text_buffer::TextBuffer;
pub use view::View;
pub use window::Window;

use crate::cell::Cell;
use crate::layout::{Position, Rect};

/// Off-screen storage for a rectangular grid of terminal cells.
///
/// A `Buffer` owns `width * height` [`Cell`] values in row-major order.
/// Row `y` lives at `cells[y * width..(y + 1) * width]`, and column `x`
/// within that row is addressed by [`Position::new`](crate::layout::Position::new)
/// through the [`Surface`] and [`SurfaceMut`] APIs.
///
/// Use a buffer when rendering into memory before presenting the result to
/// another surface, when composing with [`Window`]s, or when tests need a
/// deterministic cell grid. Writes outside the buffer bounds are ignored;
/// reads outside the bounds return `None`.
///
/// Wide cells are stored as a primary [`Cell`] followed
/// by one continuation cell. Prefer [`Buffer::set`] or
/// [`SurfaceMut::set_cell`] for writes so that continuation slots are kept
/// consistent.
#[derive(Debug, Clone)]
pub struct Buffer {
    cells: Vec<Cell>,
    width: u16,
    height: u16,
}

impl Buffer {
    /// Create a new blank buffer.
    ///
    /// The returned buffer has `width * height` cells, all initialized to
    /// [`Cell::BLANK`]. `width` and `height` are measured in terminal cell
    /// columns and rows, not bytes or grapheme clusters.
    ///
    /// # Parameters
    ///
    /// - `width`: number of columns in each row.
    /// - `height`: number of rows.
    ///
    /// # Returns
    ///
    /// A row-major [`Buffer`] with bounds `Rect::new(0, 0, width, height)`.
    ///
    /// # Panics
    ///
    /// Panics if `width * height` cannot be allocated.
    ///
    /// # Usage notes
    ///
    /// Zero-sized dimensions are valid. Accessors will then return empty
    /// rows or `None` according to the resulting bounds.
    pub fn new(width: u16, height: u16) -> Self {
        let total = (width as usize) * (height as usize);
        let cells = vec![Cell::BLANK; total];
        Self {
            cells,
            width,
            height,
        }
    }

    /// Return the buffer width in terminal cell columns.
    ///
    /// # Returns
    ///
    /// The number of columns in each row.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// This is the inherent accessor for the stored width. The [`Bounded`]
    /// implementation reports the same value through [`Bounded::width`].
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Return the buffer height in terminal cell rows.
    ///
    /// # Returns
    ///
    /// The number of rows in the buffer.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// This is the inherent accessor for the stored height. The [`Bounded`]
    /// implementation reports the same value through [`Bounded::height`].
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Borrow one row as a contiguous slice.
    ///
    /// # Parameters
    ///
    /// - `y`: zero-based row index in buffer coordinates.
    ///
    /// # Returns
    ///
    /// `Some(&[Cell])` with length [`Buffer::width`] when `y` is inside the
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
    pub fn line(&self, y: u16) -> Option<&[Cell]> {
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
    /// `Some(&mut [Cell])` with length [`Buffer::width`] when `y` is inside
    /// the buffer, or `None` when `y >= height`.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Direct slice mutation bypasses the wide-cell accounting performed by
    /// [`Buffer::set`]. It is appropriate for whole-row helpers that manage
    /// continuations themselves; prefer [`Buffer::set`] for ordinary writes.
    #[inline]
    pub fn line_mut(&mut self, y: u16) -> Option<&mut [Cell]> {
        if y >= self.height {
            return None;
        }
        let w = self.width as usize;
        let start = (y as usize) * w;
        Some(&mut self.cells[start..start + w])
    }

    /// Swap the contents of rows `a` and `b` in place.
    fn swap_rows(&mut self, a: usize, b: usize) {
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

    /// Borrow one cell mutably.
    ///
    /// # Parameters
    ///
    /// - `pos`: zero-based cell coordinate in buffer coordinates.
    ///
    /// # Returns
    ///
    /// `Some(&mut Cell)` for an in-bounds position, or `None` when `pos`
    /// lies outside the buffer.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Mutating a cell through this handle does not update neighboring
    /// continuation cells. Do not change a cell from narrow to wide, wide to
    /// narrow, or continuation to primary through this handle; use
    /// [`Buffer::set`] or [`SurfaceMut::set_cell`] when the cell width may
    /// change.
    pub fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell> {
        if pos.y >= self.height || pos.x >= self.width {
            return None;
        }
        let w = self.width as usize;
        Some(&mut self.cells[(pos.y as usize) * w + (pos.x as usize)])
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
    /// destination column is set to [`Cell::BLANK`] instead.
    ///
    /// Out-of-bounds writes are ignored.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// This is the implementation behind [`SurfaceMut::set_cell`] for
    /// `Buffer`. Use it instead of [`Buffer::cell_mut`] whenever the write
    /// might affect the width or role of neighboring cells.
    pub fn set(&mut self, pos: impl Into<Position>, cell: &Cell) {
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
                line[pc..end].fill(Cell::BLANK);
            } else if line[pc].is_continuation() {
                // Buffer is corrupted — just blank the single cell
                line[x] = Cell::BLANK;
            }
        }

        // If we're overwriting the primary cell of a wide char, blank continuations
        if line[x].is_wide() {
            let w = line[x].width() as usize;
            let end = (x + w).min(width);
            line[x + 1..end].fill(Cell::BLANK);
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
                        line[i + 1..end].fill(Cell::BLANK);
                    }
                    line[i] = Cell::continuation();
                }
            }

            // Truncate at end of line: if wide cell doesn't fit, replace with blank
            if x + cell_width > width {
                line[x] = Cell::BLANK;
                return;
            }
        }

        line[x] = cell.clone();
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
    /// row and column. Newly exposed cells are filled with [`Cell::BLANK`],
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
    pub fn resize(&mut self, width: u16, height: u16) {
        if width == self.width && height == self.height {
            return;
        }

        let new_total = (width as usize) * (height as usize);
        let mut new_cells = vec![Cell::BLANK; new_total];

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

impl Bounded for Buffer {
    fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
}

impl Surface for Buffer {
    fn cell(&self, pos: Position) -> Option<&Cell> {
        if pos.y >= self.height || pos.x >= self.width {
            return None;
        }
        let w = self.width as usize;
        Some(&self.cells[(pos.y as usize) * w + (pos.x as usize)])
    }
}

impl SurfaceMut for Buffer {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        self.set(pos, cell);
    }

    fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell> {
        Buffer::cell_mut(self, pos)
    }

    /// Fill the clipped intersection of `rect` with `cell`. For
    /// width-1 fills this collapses the trait default's per-cell
    /// `set_cell` loop into one `slice::fill` per row, with explicit
    /// wide-cell edge-straddle cleanup at the left and right boundaries
    /// so any wide cell crossing the fill region leaves no orphan
    /// primary or continuation behind. Wide fills (`cell.width() > 1`)
    /// stay on the stepped `set_cell` path so primary/continuation
    /// pairing and the trailing-partial-slot blank are placed by the
    /// same wide-cell handling that `set` already implements.
    fn fill_rect(&mut self, rect: Rect, cell: &Cell) {
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
                    self.set(Position::new(x, y), &Cell::BLANK);
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
                        *slot = Cell::BLANK;
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
                        *slot = Cell::BLANK;
                    }
                }
            }

            line[lo..hi].fill(cell.clone());
        }
    }

    fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        Buffer::insert_lines(self, y, n, bounds_bottom, fill);
    }

    fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        Buffer::delete_lines(self, y, n, bounds_bottom, fill);
    }

    fn insert_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        Buffer::insert_cells(self, pos, n, bounds_right, fill);
    }

    fn delete_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        Buffer::delete_cells(self, pos, n, bounds_right, fill);
    }
}

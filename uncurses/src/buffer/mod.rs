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
//! ```rust
//! use uncurses::buffer::{Buffer, Surface, SurfaceMut};
//! use uncurses::cell::{Cell, Content};
//! use uncurses::layout::Position;
//!
//! let mut buf = Buffer::new(4, 2);
//! buf.set_cell(Position::new(0, 0), &Cell::from('x'));
//! assert_eq!(buf.cell(Position::new(0, 0)).unwrap().content, Content::Char('x'));
//! ```
//!
//! ## Buffer storage
//!
//! [`Buffer`] stores a fixed-size grid in one row-major `Vec<Cell>`.
//! Column `x` and row `y` map to `cells[y * width + x]`, so each row is a
//! contiguous slice and cloning the buffer requires only one allocation.
//! Resizing allocates a new row-major backing store, copies the
//! intersection of the old and new extents, and fills new slots with
//! [`Cell::default`].
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
//! a continuation placeholder in the next column. [`SurfaceMut::set_cell`] writes
//! that placeholder automatically, blanks stale halves when overwriting an
//! existing wide cell, and replaces a wide cell with a blank when it would
//! not fit at the end of a row. The default [`Surface::draw`] implementation
//! preserves the same invariant when blitting between surfaces: orphan
//! continuations and clipped wide primaries are emitted as blanks.

mod ops;
mod surface;
mod view;
mod window;

pub(crate) mod grid;
#[cfg(test)]
mod tests;
mod text_buffer;

pub(crate) use grid::{Grid, GridCell, fill_line_into};
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
/// by one continuation cell. Prefer [`SurfaceMut::set_cell`] or
/// [`SurfaceMut::set_cell`] for writes so that continuation slots are kept
/// consistent.
#[derive(Debug, Clone)]
pub struct Buffer {
    grid: Grid<Cell>,
}

impl Buffer {
    /// Create a new blank buffer.
    ///
    /// The returned buffer has `width * height` cells, all initialized to
    /// [`Cell::default`]. `width` and `height` are measured in terminal cell
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
        Self {
            grid: Grid::new(width, height),
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
        self.grid.width()
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
        self.grid.height()
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
    /// row and column. Newly exposed cells are blank, and cells outside the
    /// new bounds are discarded.
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
        self.grid.resize(width, height);
    }
}

impl Bounded for Buffer {
    fn bounds(&self) -> Rect {
        self.grid.bounds()
    }
}

impl Surface for Buffer {
    fn cell(&self, pos: Position) -> Option<Cell> {
        self.grid.get(pos).cloned()
    }
}

impl SurfaceMut for Buffer {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        self.grid.set(pos, cell);
    }

    fn fill_rect(&mut self, rect: Rect, cell: &Cell) {
        self.grid.fill_rect(rect, cell);
    }

    fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.grid.insert_lines(y, n, bounds_bottom, fill);
    }

    fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.grid.delete_lines(y, n, bounds_bottom, fill);
    }

    fn insert_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        self.grid.insert_cells(pos, n, bounds_right, fill);
    }

    fn delete_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        self.grid.delete_cells(pos, n, bounds_right, fill);
    }
}

//! Geometry primitives for the cell grid.
//!
//! Three `Copy`, zero-cost wrappers describe points and extents on the
//! terminal grid:
//!
//! - [`Position`] - an `(x, y)` point.
//! - [`Size`] - a `width × height` extent.
//! - [`Rect`] - an axis-aligned rectangle, `(x, y, width, height)`.
//!
//! ## Coordinate system
//!
//! The origin is the **top-left** corner of the grid. `x` increases to the
//! right and `y` increases **downward**, so `(0, 0)` is the first cell and a
//! larger `y` is further down the screen. A [`Rect`] covers the half-open
//! ranges `x..x + width` (columns) and `y..y + height` (rows):
//!
//! ```text
//!       x:  0   1   2   3   4
//!         ┌───┬───┬───┬───┬───┐
//!   y: 0  │   │   │   │   │   │
//!         ├───┼───┼───┼───┼───┤
//!      1  │   │ ▓ │ ▓ │   │   │   Rect::new(1, 1, 2, 2)
//!         ├───┼───┼───┼───┼───┤   origin (1, 1), 2 × 2 cells,
//!      2  │   │ ▓ │ ▓ │   │   │   covering x ∈ 1..3, y ∈ 1..3
//!         ├───┼───┼───┼───┼───┤
//!      3  │   │   │   │   │   │
//!         └───┴───┴───┴───┴───┘
//! ```
//!
//! ## Tuple shorthand
//!
//! Most positional APIs accept `impl Into<Position>`, `impl Into<Size>`, and
//! `impl Into<Rect>`, so plain tuples work as ergonomic shorthand:
//!
//! ```
//! use uncurses::layout::{Position, Rect};
//!
//! let p: Position = (3, 5).into();
//! assert_eq!(p, Position::new(3, 5));
//!
//! let r: Rect = (3, 5, 10, 2).into();
//! assert_eq!(r, Rect::new(3, 5, 10, 2));
//! ```

mod position;
mod rect;
mod size;

pub use position::Position;
pub use rect::Rect;
pub use size::Size;

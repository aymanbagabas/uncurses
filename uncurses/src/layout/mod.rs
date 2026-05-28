//! Geometry primitives for the cell grid.
//!
//! [`Position`] is a `(x, y)` point and [`Rect`] is an axis-aligned
//! rectangle with `(x, y, width, height)`. The terminal coordinate
//! system has its origin at the top-left corner of the grid; `x`
//! increases to the right and `y` increases downward. Both types are
//! zero-cost wrappers and implement `Copy`.
//!
//! Most positional APIs in this crate accept `impl Into<Position>` and
//! `impl Into<Rect>`, so plain tuples work as ergonomic shorthand:
//!
//! ```
//! use uncurses::{Position, Rect};
//!
//! let p: Position = (3, 5).into();
//! assert_eq!(p, Position::new(3, 5));
//!
//! let r: Rect = (3, 5, 10, 2).into();
//! assert_eq!(r, Rect::new(3, 5, 10, 2));
//! ```

mod position;
mod rect;

pub use position::Position;
pub use rect::Rect;

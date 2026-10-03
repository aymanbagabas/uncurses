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

/// Whether a run `w` columns wide, placed at column `x`, reaches past the
/// exclusive right edge `right`.
///
/// A cell can claim up to 255 columns, and [`Rect::right`] saturates, so a
/// row near the end of the address space can put `x` close enough to
/// `u16::MAX` that `x + w` does not fit in a `u16`. A run that cannot be
/// addressed runs past any edge, so an overflow answers yes.
///
/// Saturating the sum instead would answer no at the last addressable
/// column, because `u16::MAX > u16::MAX` is false, and let a write land
/// outside the edge it was checked against.
pub(crate) fn overruns(x: u16, w: u16, right: u16) -> bool {
    x.checked_add(w).is_none_or(|end| end > right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overruns_reports_an_overflowing_placement_as_past_the_edge() {
        assert!(!overruns(0, 2, 10));
        assert!(!overruns(8, 2, 10));
        assert!(overruns(9, 2, 10));
        assert!(overruns(u16::MAX, 1, u16::MAX));
        assert!(overruns(u16::MAX - 1, 255, u16::MAX));
    }
}

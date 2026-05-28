//! [`Position`] — a point in the cell grid.

use core::fmt;

use super::Rect;

/// A point in the cell grid (`x` column, `y` row).
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct Position {
    /// Column (0-based, increases to the right).
    pub x: u16,
    /// Row (0-based, increases downward).
    pub y: u16,
}

impl Position {
    /// The top-left corner of the grid.
    pub const ORIGIN: Self = Self::new(0, 0);

    /// Create a position from `(x, y)`.
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

impl From<(u16, u16)> for Position {
    fn from((x, y): (u16, u16)) -> Self {
        Self { x, y }
    }
}

impl From<Position> for (u16, u16) {
    fn from(p: Position) -> Self {
        (p.x, p.y)
    }
}

impl From<Rect> for Position {
    /// The top-left corner of the rectangle.
    fn from(r: Rect) -> Self {
        Self { x: r.x, y: r.y }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_from_tuple() {
        let p: Position = (3, 5).into();
        assert_eq!(p, Position::new(3, 5));
        let (x, y): (u16, u16) = p.into();
        assert_eq!((x, y), (3, 5));
    }

    #[test]
    fn position_from_rect() {
        let r = Rect::new(7, 9, 1, 1);
        assert_eq!(Position::from(r), Position::new(7, 9));
    }
}

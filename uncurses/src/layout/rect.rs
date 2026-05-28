//! [`Rect`] — an axis-aligned rectangle in the cell grid.

use core::fmt;

use super::Position;

/// An axis-aligned rectangle in the cell grid.
///
/// The rectangle is anchored at its top-left corner `(x, y)` and extends
/// `width` cells to the right and `height` cells down. The right and
/// bottom edges are exclusive: a cell at `(x, y)` is contained iff
/// `left() <= x < right()` and `top() <= y < bottom()`.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Rect {
    /// Column of the top-left corner.
    pub x: u16,
    /// Row of the top-left corner.
    pub y: u16,
    /// Width in cells.
    pub width: u16,
    /// Height in cells.
    pub height: u16,
}

impl Rect {
    /// An empty rectangle at the origin.
    pub const ZERO: Self = Self {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };

    /// Create a rectangle from `(x, y, width, height)`.
    ///
    /// `width` and `height` are clamped so that `right()` and `bottom()`
    /// stay within `u16`.
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        let width = x.saturating_add(width) - x;
        let height = y.saturating_add(height) - y;
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Area in cells.
    pub const fn area(self) -> u32 {
        (self.width as u32) * (self.height as u32)
    }

    /// True when the rectangle has zero area.
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Inclusive left edge (column of the leftmost cell).
    pub const fn left(self) -> u16 {
        self.x
    }

    /// Exclusive right edge (one past the rightmost column).
    pub const fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    /// Inclusive top edge (row of the topmost cell).
    pub const fn top(self) -> u16 {
        self.y
    }

    /// Exclusive bottom edge (one past the bottommost row).
    pub const fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }

    /// The top-left corner as a [`Position`].
    pub const fn position(self) -> Position {
        Position {
            x: self.x,
            y: self.y,
        }
    }

    /// True when `pos` lies inside the rectangle.
    pub fn contains(self, pos: impl Into<Position>) -> bool {
        let p = pos.into();
        p.x >= self.left() && p.x < self.right() && p.y >= self.top() && p.y < self.bottom()
    }

    /// The largest rectangle contained in both `self` and `other`.
    pub fn intersection(self, other: Self) -> Self {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = self.right().min(other.right());
        let y2 = self.bottom().min(other.bottom());
        if x2 <= x1 || y2 <= y1 {
            Self::ZERO
        } else {
            Self {
                x: x1,
                y: y1,
                width: x2 - x1,
                height: y2 - y1,
            }
        }
    }

    /// The smallest rectangle that contains both `self` and `other`.
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x1 = self.x.min(other.x);
        let y1 = self.y.min(other.y);
        let x2 = self.right().max(other.right());
        let y2 = self.bottom().max(other.bottom());
        Self {
            x: x1,
            y: y1,
            width: x2 - x1,
            height: y2 - y1,
        }
    }
}

impl From<(u16, u16, u16, u16)> for Rect {
    fn from((x, y, width, height): (u16, u16, u16, u16)) -> Self {
        Self::new(x, y, width, height)
    }
}

impl From<Rect> for (u16, u16, u16, u16) {
    fn from(r: Rect) -> Self {
        (r.x, r.y, r.width, r.height)
    }
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}+{}+{}", self.width, self.height, self.x, self.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_basics() {
        let r = Rect::new(2, 3, 10, 4);
        assert_eq!(r.left(), 2);
        assert_eq!(r.top(), 3);
        assert_eq!(r.right(), 12);
        assert_eq!(r.bottom(), 7);
        assert_eq!(r.area(), 40);
        assert!(!r.is_empty());
        assert!(r.contains((2, 3)));
        assert!(r.contains((11, 6)));
        assert!(!r.contains((12, 6)));
        assert!(!r.contains((11, 7)));
    }

    #[test]
    fn rect_new_saturates() {
        let r = Rect::new(u16::MAX - 2, 0, 100, 1);
        assert_eq!(r.right(), u16::MAX);
        assert_eq!(r.width, 2);
    }

    #[test]
    fn rect_intersection() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersection(b), Rect::new(5, 5, 5, 5));

        let disjoint = Rect::new(20, 20, 5, 5);
        assert!(a.intersection(disjoint).is_empty());
    }

    #[test]
    fn rect_union() {
        let a = Rect::new(0, 0, 5, 5);
        let b = Rect::new(10, 10, 5, 5);
        assert_eq!(a.union(b), Rect::new(0, 0, 15, 15));
        assert_eq!(a.union(Rect::ZERO), a);
        assert_eq!(Rect::ZERO.union(b), b);
    }
}

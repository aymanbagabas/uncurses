//! [`Size`] — a width and height in the cell grid.

use core::fmt;

use super::Rect;

/// A size in the cell grid (`width` columns by `height` rows).
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct Size {
    /// Width in cells (columns).
    pub width: u16,
    /// Height in cells (rows).
    pub height: u16,
}

impl Size {
    /// A zero-sized area.
    pub const ZERO: Self = Self::new(0, 0);

    /// Create a size from `(width, height)`.
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

impl From<(u16, u16)> for Size {
    fn from((width, height): (u16, u16)) -> Self {
        Self { width, height }
    }
}

impl From<Size> for (u16, u16) {
    fn from(s: Size) -> Self {
        (s.width, s.height)
    }
}

impl From<Rect> for Size {
    /// The rectangle's `width` by `height`, dropping its position.
    fn from(r: Rect) -> Self {
        Self {
            width: r.width,
            height: r.height,
        }
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_from_tuple() {
        let s: Size = (80, 24).into();
        assert_eq!(s, Size::new(80, 24));
        let (w, h): (u16, u16) = s.into();
        assert_eq!((w, h), (80, 24));
    }

    #[test]
    fn size_from_rect() {
        let r = Rect::new(7, 9, 10, 2);
        assert_eq!(Size::from(r), Size::new(10, 2));
    }
}

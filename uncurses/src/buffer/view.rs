//! Sub-rectangle borrows over any surface.
//!
//! A [`View`] is a thin handle that narrows access to a fixed
//! sub-rect of an existing [`Surface`] / [`SurfaceMut`]. Reads and
//! writes outside the view's [`Bounded::bounds`] are silently
//! ignored. Coordinates passed to [`Surface::cell`] /
//! [`SurfaceMut::set_cell`] are in the *inner* surface's coordinate
//! space — a view does **not** translate addresses, it just clips
//! them.

use crate::cell::Cell;
use crate::layout::{Position, Rect};

use super::{Bounded, Surface, SurfaceMut};

/// Borrowed sub-rect over an existing surface.
#[derive(Debug)]
pub struct View<'a, T: ?Sized> {
    inner: &'a mut T,
    bounds: Rect,
}

impl<'a, T: Bounded + ?Sized> View<'a, T> {
    /// New view bound to the intersection of `bounds` and
    /// `inner.bounds()`. If they are disjoint, the resulting view is
    /// empty.
    pub fn new(inner: &'a mut T, bounds: impl Into<Rect>) -> Self {
        let bounds = inner.bounds().intersection(bounds.into());
        Self { inner, bounds }
    }

    /// Reborrow the inner surface mutably. Useful when the view
    /// itself is borrowed but the caller needs to escape the clip.
    pub fn inner_mut(&mut self) -> &mut T {
        self.inner
    }
}

impl<T: Bounded + ?Sized> Bounded for View<'_, T> {
    fn bounds(&self) -> Rect {
        self.bounds
    }
}

impl<T: Surface + ?Sized> Surface for View<'_, T> {
    fn cell(&self, pos: Position) -> Option<&Cell> {
        if self.bounds.contains(pos) {
            self.inner.cell(pos)
        } else {
            None
        }
    }
}

impl<T: SurfaceMut + ?Sized> SurfaceMut for View<'_, T> {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        if self.bounds.contains(pos) {
            self.inner.set_cell(pos, cell);
        }
    }

    fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell> {
        if self.bounds.contains(pos) {
            self.inner.cell_mut(pos)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::cell::Cell;

    #[test]
    fn view_clips_writes_outside_bounds() {
        let mut buf = Buffer::new(10, 5);
        {
            let mut v = View::new(&mut buf, Rect::new(2, 1, 3, 2));
            v.set_cell(Position::new(2, 1), &Cell::narrow("A")); // inside
            v.set_cell(Position::new(0, 0), &Cell::narrow("B")); // outside view
            v.set_cell(Position::new(5, 1), &Cell::narrow("C")); // right of view
        }
        assert_eq!(buf.cell(Position::new(2, 1)).unwrap().content(), "A");
        assert!(buf.cell(Position::new(0, 0)).unwrap() == &Cell::BLANK);
        assert!(buf.cell(Position::new(5, 1)).unwrap() == &Cell::BLANK);
    }

    #[test]
    fn view_clips_reads_outside_bounds() {
        let mut buf = Buffer::new(10, 5);
        buf.set_cell(Position::new(2, 1), &Cell::narrow("A"));
        buf.set_cell(Position::new(0, 0), &Cell::narrow("B"));
        let mut buf2 = buf.clone();
        let v = View::new(&mut buf2, Rect::new(2, 1, 3, 2));
        assert_eq!(v.cell(Position::new(2, 1)).unwrap().content(), "A");
        assert!(v.cell(Position::new(0, 0)).is_none());
    }

    #[test]
    fn view_clips_to_inner_bounds() {
        let mut buf = Buffer::new(5, 3);
        let v = View::new(&mut buf, Rect::new(3, 1, 10, 10));
        // Clipped down to fit inside the 5x3 buffer.
        assert_eq!(v.bounds(), Rect::new(3, 1, 2, 2));
    }
}

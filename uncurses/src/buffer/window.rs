//! Owned windows — a [`Buffer`] plus the placement metadata needed to
//! blit it into a larger surface.
//!
//! A [`Window`] holds its own cells and knows where on a target it
//! should land ([`Window::position`]). It is the unit of composition
//! when an app stitches several independent panes into one frame:
//! every window writes into its own buffer in window-local
//! coordinates, then [`Window::present`] copies its cells to a target
//! at the configured position.

use crate::cell::Cell;
use crate::layout::{Position, Rect};

use super::{Bounded, Buffer, Surface, SurfaceMut};

/// A self-contained pane: a [`Buffer`] plus its position on a target
/// surface.
#[derive(Debug, Clone)]
pub struct Window {
    buffer: Buffer,
    position: Position,
}

impl Window {
    /// New `width × height` window at the origin.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            buffer: Buffer::new(width, height),
            position: Position::new(0, 0),
        }
    }

    /// Builder-style setter for [`Self::position`].
    pub fn with_position(mut self, pos: impl Into<Position>) -> Self {
        self.position = pos.into();
        self
    }

    /// Where the window's top-left lands on a target surface.
    pub fn position(&self) -> Position {
        self.position
    }

    /// Move the window. Affects subsequent [`Self::present`] calls.
    pub fn set_position(&mut self, pos: impl Into<Position>) {
        self.position = pos.into();
    }

    /// Resize the underlying buffer in place.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.buffer.resize(width, height);
    }

    /// Read-only handle to the underlying buffer.
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Mutable handle to the underlying buffer.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    /// Blit this window's cells onto `target`, mapping the window's
    /// top-left to [`Self::position`] in target coordinates.
    pub fn present<T: SurfaceMut + ?Sized>(&self, target: &mut T) {
        self.buffer.draw(target, self.position);
    }
}

impl Bounded for Window {
    fn bounds(&self) -> Rect {
        self.buffer.bounds()
    }
}

impl Surface for Window {
    fn cell(&self, pos: Position) -> Option<&Cell> {
        self.buffer.cell(pos)
    }
}

impl SurfaceMut for Window {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        self.buffer.set_cell(pos, cell);
    }

    fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell> {
        self.buffer.cell_mut(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;

    #[test]
    fn write_then_present_offsets_by_position() {
        let mut src = Window::new(3, 2).with_position((5, 1));
        src.set_cell(Position::new(0, 0), &Cell::narrow("A"));
        src.set_cell(Position::new(2, 1), &Cell::narrow("B"));

        let mut dst = Buffer::new(10, 4);
        src.present(&mut dst);

        assert_eq!(dst.cell(Position::new(5, 1)).unwrap().content(), "A");
        assert_eq!(dst.cell(Position::new(7, 2)).unwrap().content(), "B");
        // Outside the window's footprint stays blank.
        assert!(dst.cell(Position::new(0, 0)).unwrap().is_blank());
    }

    #[test]
    fn present_clips_to_target_bounds() {
        let mut src = Window::new(4, 4).with_position((8, 2));
        src.fill(&Cell::narrow("X"));
        // Target is 10x3 — only the top-left 2x1 of src lands.
        let mut dst = Buffer::new(10, 3);
        src.present(&mut dst);
        assert_eq!(dst.cell(Position::new(8, 2)).unwrap().content(), "X");
        assert_eq!(dst.cell(Position::new(9, 2)).unwrap().content(), "X");
        // Below target bottom — never written.
        assert!(dst.cell(Position::new(8, 0)).unwrap().is_blank());
    }
}

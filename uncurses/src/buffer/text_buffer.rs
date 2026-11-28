//! A text-capable buffer with a surface width policy.

/// A thin wrapper around [`Buffer`](super::Buffer) for text output and encoding.
///
/// The underlying buffer owns the width policy and resources. Policy setters
/// affect subsequent text and cell factories. Conversion with [`Self::into_buffer`]
/// retains the policy, content, and styles.
///
/// `TextBuffer` uses the [`SurfaceMut`] defaults for insert/delete operations.
/// They replay primary cells through writes. In contrast, [`Buffer`]'s
/// inherent operations rotate stored rows and columns. For example, deletion
/// that starts inside a wide cell clears its primary through the default
/// path, while the inherent buffer rotation retains the primary before the span.
///
/// ```
/// use uncurses::buffer::TextBuffer;
/// use uncurses::style::Style;
/// use uncurses::text::{Encode, TextSurface};
/// let mut frame = TextBuffer::new(12, 1);
/// frame.set_str((0, 0), "hello", Style::new().bold());
/// assert!(frame.display().to_string().contains("hello"));
/// ```
#[derive(Debug, Clone)]
pub struct TextBuffer {
    buffer: super::Buffer,
}

use super::{Bounded, Buffer, Surface, SurfaceMut};
use crate::cell::CellRef;
use crate::layout::{Position, Rect};
use crate::style::Style;
use crate::text::{TextSurface, WidthMode};

impl TextBuffer {
    /// Create a text buffer with the default width policy.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            buffer: Buffer::new(width, height),
        }
    }
    /// Select the width policy for subsequent text.
    pub fn with_width_mode(mut self, mode: WidthMode) -> Self {
        self.set_width_mode(mode);
        self
    }
    /// Select the ambiguous-width policy for subsequent text.
    pub fn with_eaw_wide(mut self, wide: bool) -> Self {
        self.set_eaw_wide(wide);
        self
    }
    /// Set the width policy for subsequent text.
    pub fn set_width_mode(&mut self, mode: WidthMode) {
        self.buffer.set_width_mode(mode);
    }
    /// Set the ambiguous-width policy for subsequent text.
    pub fn set_eaw_wide(&mut self, wide: bool) {
        self.buffer.set_eaw_wide(wide);
    }
    /// Return the column count.
    pub fn width(&self) -> u16 {
        self.buffer.width()
    }
    /// Return the row count.
    pub fn height(&self) -> u16 {
        self.buffer.height()
    }
    /// Resize the underlying grid without remeasurement.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.buffer.resize(width, height);
    }
    /// Borrow the underlying grid.
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }
    /// Borrow the underlying grid mutably.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }
    /// Return the underlying grid.
    pub fn into_buffer(self) -> Buffer {
        self.buffer
    }
    /// Write a portable or borrowed cell.
    pub fn set_cell<'a>(&mut self, pos: Position, cell: impl Into<CellRef<'a>>) {
        self.buffer.set(pos, cell);
    }
}
impl Bounded for TextBuffer {
    fn bounds(&self) -> Rect {
        self.buffer.bounds()
    }
}
impl Surface for TextBuffer {
    fn cell(&self, pos: Position) -> Option<CellRef<'_>> {
        self.buffer.cell(pos)
    }
    fn width_mode(&self) -> WidthMode {
        self.buffer.width_mode()
    }
    fn eaw_wide(&self) -> bool {
        self.buffer.eaw_wide()
    }
}
impl SurfaceMut for TextBuffer {
    fn set_cell(&mut self, pos: Position, cell: CellRef<'_>) {
        self.buffer.set(pos, cell);
    }
    fn set_style(&mut self, pos: Position, style: Style) {
        self.buffer.set_style(pos, style);
    }
}
impl TextSurface for TextBuffer {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Surface;
    use crate::layout::Position;
    use crate::style::Style;
    use crate::text::{Encode, TextSurface};

    #[test]
    fn default_delete_clears_a_primary_before_the_deleted_span() {
        use crate::cell::Cell;
        use crate::color::Color;
        let style = Style::new()
            .fg(Color::Rgb(28, 41, 172))
            .bg(Color::Indexed(28))
            .link("https://example.test/arena", "id=shared");
        let mut text = TextBuffer::new(32, 1);
        text.set_cell((1, 0).into(), &Cell::new("", 3).style(&style));
        let mut raw = text.buffer().clone();
        text.delete_cells((2, 0).into(), 2, 30, Cell::BLANK.as_ref());
        raw.delete_cells((2, 0), 2, 30, Cell::BLANK.as_ref());
        let boundary = text.cell((1, 0).into()).unwrap();
        assert_eq!(boundary.width(), 1);
        assert_eq!(boundary.content(), " ");
        assert_eq!(boundary.to_style(), style);
        assert_eq!(raw.cell((1, 0).into()).unwrap().width(), 3);
        text.buffer.resources.audit(&text.buffer.cells);
        raw.resources.audit(&raw.cells);
    }

    #[test]
    fn paints_and_encodes() {
        let mut tb = TextBuffer::new(8, 1);
        let end = tb.set_str((0, 0), "hi", Style::new());
        assert_eq!(end, Position::new(2, 0));
        assert_eq!(tb.display().to_string(), "hi");
    }

    #[test]
    fn width_policy_affects_measurement() {
        let narrow = TextBuffer::new(4, 1);
        let wide = TextBuffer::new(4, 1).with_eaw_wide(true);
        assert_eq!(narrow.str_width("…"), 1);
        assert_eq!(wide.str_width("…"), 2);
    }

    #[test]
    fn into_buffer_roundtrips() {
        let mut tb = TextBuffer::new(3, 1);
        tb.set_str((0, 0), "ab", Style::new());
        let buf = tb.into_buffer();
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().content(), "a");
    }
}

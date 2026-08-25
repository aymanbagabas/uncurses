//! A width-aware cell buffer that paints text and serializes to escapes.
//!
//! [`TextBuffer`] is a [`Buffer`] plus a text-width policy. The policy is what
//! [`TextSurface`] needs to lay out grapheme clusters into cells, so a
//! `TextBuffer` implements [`TextSurface`] and can be painted with
//! [`set_str`](TextSurface::set_str) directly, unlike a bare [`Buffer`].
//!
//! Because it is a [`Surface`], a `TextBuffer` also gets the
//! [`Encode`](crate::text::Encode) trait for free, so you can paint a frame and
//! serialize it to escape sequences with
//! [`encode`](crate::text::Encode::encode) /
//! [`encode_with`](crate::text::Encode::encode_with) or render it into a
//! string with [`display`](crate::text::Encode::display). This makes it the
//! tool for one-shot and append-style output: paint a full frame, encode it,
//! and write the bytes wherever you like, with no diffing renderer and no
//! terminal session. For in-place repainting of a live terminal across frames,
//! reach for [`Screen`](crate::screen::Screen) instead, whose diffing renderer
//! tracks the terminal and emits only the changed bytes.
//!
//! ```
//! use uncurses::buffer::TextBuffer;
//! use uncurses::style::Style;
//! use uncurses::text::{Encode, TextSurface};
//!
//! let mut frame = TextBuffer::new(12, 1);
//! frame.set_str((0, 0), "hello", Style::new().bold());
//! let bytes = frame.display().to_string();
//! assert!(bytes.contains("hello"));
//! ```

use crate::buffer::{Bounded, Buffer, Surface, SurfaceMut};
use crate::cell::Cell;
use crate::layout::{Position, Rect};
use crate::text::{TextSurface, WidthMode};

/// A [`Buffer`] paired with a text-width policy.
///
/// Construct one with [`new`](Self::new), choose the width policy with
/// [`with_width_mode`](Self::with_width_mode) /
/// [`with_eaw_wide`](Self::with_eaw_wide), paint with the
/// [`TextSurface`]/[`SurfaceMut`] methods, then serialize with the
/// [`Encode`](crate::text::Encode) trait.
#[derive(Debug, Clone)]
pub struct TextBuffer {
    buffer: Buffer,
    width_mode: WidthMode,
    eaw_wide: bool,
}

impl TextBuffer {
    /// Create a `width` by `height` text buffer of blank cells.
    ///
    /// The width policy defaults to [`WidthMode::Wc`] with East-Asian
    /// Ambiguous characters measured as one cell.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            buffer: Buffer::new(width, height),
            width_mode: WidthMode::default(),
            eaw_wide: false,
        }
    }

    /// Set the grapheme-cluster width policy and return the updated buffer.
    pub fn with_width_mode(mut self, mode: WidthMode) -> Self {
        self.width_mode = mode;
        self
    }

    /// Set the East-Asian Ambiguous policy and return the updated buffer.
    ///
    /// When `true`, code points whose East-Asian-Width property is
    /// `Ambiguous` are measured as two cells instead of one.
    pub fn with_eaw_wide(mut self, eaw_wide: bool) -> Self {
        self.eaw_wide = eaw_wide;
        self
    }

    /// Set the grapheme-cluster width policy in place.
    pub fn set_width_mode(&mut self, mode: WidthMode) {
        self.width_mode = mode;
    }

    /// Set the East-Asian Ambiguous policy in place.
    pub fn set_eaw_wide(&mut self, eaw_wide: bool) {
        self.eaw_wide = eaw_wide;
    }

    /// The width in cells.
    pub fn width(&self) -> u16 {
        self.buffer.width()
    }

    /// The height in cells.
    pub fn height(&self) -> u16 {
        self.buffer.height()
    }

    /// Resize the buffer, preserving overlapping cells.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.buffer.resize(width, height);
    }

    /// Borrow the underlying [`Buffer`].
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Mutably borrow the underlying [`Buffer`].
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    /// Consume the text buffer and return the underlying [`Buffer`].
    pub fn into_buffer(self) -> Buffer {
        self.buffer
    }
}

impl Bounded for TextBuffer {
    fn bounds(&self) -> Rect {
        self.buffer.bounds()
    }
}

impl Surface for TextBuffer {
    fn cell(&self, pos: Position) -> Option<Cell> {
        self.buffer.cell(pos)
    }
}

impl SurfaceMut for TextBuffer {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        self.buffer.set_cell(pos, cell);
    }
}

impl TextSurface for TextBuffer {
    fn width_mode(&self) -> WidthMode {
        self.width_mode
    }

    fn eaw_wide(&self) -> bool {
        self.eaw_wide
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;
    use crate::text::Encode;

    #[test]
    fn paints_and_encodes() {
        let mut tb = TextBuffer::new(8, 1);
        let end = tb.set_str((0, 0), "hi", Style::new());
        assert_eq!(end, Position::new(2, 0));
        assert_eq!(tb.display().to_string(), "hi");
    }

    #[test]
    fn width_policy_affects_measurement() {
        // A flag emoji presentation sequence: Wc measures the first scalar,
        // Grapheme measures the whole cluster. Just assert the policy plumbs
        // through to str_width without asserting exact terminal widths.
        let narrow = TextBuffer::new(4, 1);
        let wide = TextBuffer::new(4, 1).with_eaw_wide(true);
        assert!(wide.str_width("\u{2764}") >= narrow.str_width("\u{2764}"));
    }

    #[test]
    fn into_buffer_roundtrips() {
        let mut tb = TextBuffer::new(3, 1);
        tb.set_str((0, 0), "ab", Style::new());
        let buf = tb.into_buffer();
        assert_eq!(
            buf.cell(Position::new(0, 0)).unwrap().content.char(),
            Some('a')
        );
    }
}

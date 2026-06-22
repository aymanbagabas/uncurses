//! Cursor shape selection for the [`Screen`](super::Screen) facade.
//!
//! The facade exposes the cursor as a shape plus a separate blinking flag,
//! rather than the underlying DECSCUSR codes which interleave the two. A
//! shape and blinking flag map to one of the underlying cursor styles.

use crate::ansi::cursor::CursorStyle;

/// The visual shape of the text cursor, independent of whether it blinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CursorShape {
    /// A full-cell block.
    #[default]
    Block,
    /// A horizontal underline at the bottom of the cell.
    Underline,
    /// A vertical bar at the left of the cell.
    Bar,
}

impl CursorShape {
    /// Map a shape and blinking flag to the underlying cursor style.
    pub(super) fn style(self, blinking: bool) -> CursorStyle {
        match (self, blinking) {
            (CursorShape::Block, true) => CursorStyle::BlinkingBlock,
            (CursorShape::Block, false) => CursorStyle::SteadyBlock,
            (CursorShape::Underline, true) => CursorStyle::BlinkingUnderline,
            (CursorShape::Underline, false) => CursorStyle::SteadyUnderline,
            (CursorShape::Bar, true) => CursorStyle::BlinkingBar,
            (CursorShape::Bar, false) => CursorStyle::SteadyBar,
        }
    }
}

//! Cursor shape selection for the [`Program`](super::Program) facade.
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
    pub fn style(self, blinking: bool) -> CursorStyle {
        match (self, blinking) {
            (CursorShape::Block, true) => CursorStyle::BlinkingBlock,
            (CursorShape::Block, false) => CursorStyle::SteadyBlock,
            (CursorShape::Underline, true) => CursorStyle::BlinkingUnderline,
            (CursorShape::Underline, false) => CursorStyle::SteadyUnderline,
            (CursorShape::Bar, true) => CursorStyle::BlinkingBar,
            (CursorShape::Bar, false) => CursorStyle::SteadyBar,
        }
    }

    /// Read a cursor style back as a shape and blinking flag, the terms
    /// [`Program::set_cursor_style`](super::Program::set_cursor_style) takes.
    ///
    /// # Returns
    ///
    /// `None` for [`CursorStyle::Default`], which names the terminal's own
    /// choice rather than a shape, so there is nothing to report.
    pub fn from_style(style: CursorStyle) -> Option<(Self, bool)> {
        Some(match style {
            CursorStyle::Default => return None,
            CursorStyle::BlinkingBlock => (CursorShape::Block, true),
            CursorStyle::SteadyBlock => (CursorShape::Block, false),
            CursorStyle::BlinkingUnderline => (CursorShape::Underline, true),
            CursorStyle::SteadyUnderline => (CursorShape::Underline, false),
            CursorStyle::BlinkingBar => (CursorShape::Bar, true),
            CursorStyle::SteadyBar => (CursorShape::Bar, false),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_and_blinking_round_trip_through_a_style() {
        for shape in [CursorShape::Block, CursorShape::Underline, CursorShape::Bar] {
            for blinking in [true, false] {
                let style = shape.style(blinking);
                assert_eq!(CursorShape::from_style(style), Some((shape, blinking)));
            }
        }
    }

    #[test]
    fn the_terminal_default_names_no_shape() {
        // DECSCUSR 0 defers to the terminal, so there is no shape or blink
        // state to report back.
        assert_eq!(CursorShape::from_style(CursorStyle::Default), None);
    }
}

pub mod segment;

pub use segment::graphemes;

use compact_str::CompactString;

use crate::style::Style;

/// A single terminal cell.
#[derive(Debug, Clone)]
pub struct Cell {
    /// The grapheme cluster content. Empty string for a wide-cell
    /// continuation placeholder.
    content: CompactString,
    /// Visual style: colors, attributes, underline, and any attached
    /// hyperlink. The link inside `style` is reference-counted so a
    /// run of identically-linked cells shares a single allocation
    /// without per-cell deep clones.
    style: Style,
    /// Display width: 1 for normal, 2 for wide, 0 for wide-char
    /// continuation. Pairs with [`Cell::content`]: width `0` always
    /// implies an empty content string.
    width: u8,
}

impl PartialEq for Cell {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width && self.style == other.style && self.content == other.content
    }
}

impl Eq for Cell {}

impl Default for Cell {
    fn default() -> Self {
        Self::BLANK
    }
}

impl Cell {
    /// A blank cell with default style and width 1.
    pub const BLANK: Cell = Cell {
        content: CompactString::const_new(" "),
        style: Style::EMPTY,
        width: 1,
    };

    /// Create a new cell with the given grapheme-cluster `content` and
    /// display `width`.
    ///
    /// `width` is the cell count the content occupies on screen: 1 for
    /// normal, 2 for wide (CJK / wide emoji), 0 for the placeholder
    /// continuation that follows a wide cell (use `""` as content).
    /// Callers that already know the width (e.g. after running
    /// [`crate::text::grapheme_cells`]) pass it directly; callers that
    /// need to measure pass
    /// [`crate::text::char_width`] or
    /// [`crate::text::grapheme_width`] along with their chosen
    /// `eaw_wide` policy.
    pub fn new(content: impl Into<CompactString>, width: u8) -> Self {
        Cell {
            content: content.into(),
            style: Style::EMPTY,
            width,
        }
    }

    /// Whether this cell is a blank/space.
    pub fn is_blank(&self) -> bool {
        self.content.is_empty() || self.content == " " || (self.width == 0)
    }

    /// Whether this is a wide-char continuation cell.
    pub fn is_continuation(&self) -> bool {
        self.width == 0
    }

    /// The cell's grapheme-cluster content. Empty for a wide-cell
    /// continuation placeholder.
    #[inline]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }

    /// The cell's display width: 1 for normal, 2 for wide, 0 for the
    /// continuation placeholder that follows a wide cell.
    #[inline]
    pub fn width(&self) -> u8 {
        self.width
    }

    /// The cell's style (colors, attributes, underline, and any
    /// attached hyperlink).
    #[inline]
    pub fn style(&self) -> &Style {
        &self.style
    }

    /// Return a copy of this cell with the given style.
    pub fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blank_cell() {
        let c = Cell::BLANK;
        assert_eq!(c.width, 1);
        assert!(c.is_blank());
        assert!(c.style().is_empty());
    }

    #[test]
    fn test_new_cell() {
        let c = Cell::new("A", 1);
        assert_eq!(c.content(), "A");
        assert_eq!(c.width, 1);
    }

    #[test]
    fn test_continuation_cell() {
        let c = Cell::new("", 0);
        assert_eq!(c.width, 0);
        assert!(c.is_continuation());
    }

    #[test]
    fn test_cell_with_style() {
        let c = Cell::new("x", 1).with_style(Style::EMPTY.with_bold());
        assert!(c.style().attrs.contains(crate::style::AttrFlags::BOLD));
    }
}

//! Content hashing shared by the frame and scroll stages.

use crate::buffer::CellRow;
use std::hash::Hasher;

use rustc_hash::FxHasher;

/// Hash a line's content (ignoring style) for scroll detection.
///
/// Uses a non-cryptographic hasher because the only consequence of an
/// unlucky collision is a missed scroll opportunity — the affected
/// rows fall through to direct redraw, never visual corruption.
pub(crate) fn hash_line(line: CellRow<'_>) -> u64 {
    let mut hasher = FxHasher::default();
    for cell in line.iter() {
        // Match str's delimiter without validating scalar UTF-8 again.
        hasher.write(cell.content_bytes());
        hasher.write_u8(0xff);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::color::Color;
    use crate::renderer::RenderBuffer;
    use crate::style::Style;
    use std::hash::Hash;

    #[test]
    fn packed_content_hash_matches_string_hash_for_all_content_forms() {
        let cells: Vec<_> = ["", "a", "é", "中", "🦀", "e\u{301}", ""]
            .into_iter()
            .zip([0, 1, 1, 2, 2, 1, 3])
            .map(|(text, width)| Cell::new(text, width))
            .collect();
        let mut expected = FxHasher::default();
        for cell in &cells {
            cell.content().hash(&mut expected);
        }
        let owned = CellRow::from_cells(&cells);
        let mut packed = RenderBuffer::new(cells.len() as u16, 1);
        packed.line_mut(0).unwrap().copy_from(owned);
        assert_eq!(hash_line(owned), expected.finish());
        assert_eq!(hash_line(packed.line(0).unwrap()), expected.finish());
    }

    #[test]
    fn row_hash_resolves_content_and_ignores_style() {
        let mut first = RenderBuffer::new(4, 1);
        let mut second = RenderBuffer::new(4, 1);
        for (x, text) in [(0, "a\u{301}"), (1, "b\u{301}")] {
            first.set_cell((x, 0), &Cell::new(text, 1));
        }
        for (x, text) in [(1, "b\u{301}"), (0, "a\u{301}")] {
            second.set_cell(
                (x, 0),
                &Cell::new(text, 1).style(Style::default().fg(Color::Red)),
            );
        }
        let first = first.line(0).unwrap();
        let second = second.line(0).unwrap();
        assert_ne!(first, second);
        assert_eq!(hash_line(first), hash_line(second));
    }
}

//! Front → back buffer sync with per-cell equality filtering.
//!
//! Screens write freely into their `front_buf`; `fill_rect` and bulk
//! ops mark whole rows as touched even when the resulting content
//! matches the previous frame. [`Renderer::sync_front`] copies those
//! touched spans into [`Renderer::back_buf`]. Complete spans use packed
//! copies or cached arena imports; partial clusters use [`RenderBuffer::set_cell`].
//! Both paths preserve change filtering. The diff pass then
//! visits only rows that genuinely changed on the wire.
//!
//! Only the cells inside `front_buf.touched(y)` spans are read — no
//! full-buffer scan.

use crate::renderer::Renderer;
use crate::renderer::buffer::{RenderBuffer, TouchedSpan};

use crate::buffer::{CellRow, Surface};
use crate::cell::storage::PackedCell;

fn complete_clusters(cells: &[PackedCell]) -> bool {
    let mut remaining = 0;
    for cell in cells {
        match (remaining, cell.width()) {
            (0, 0) => return false,
            (0, width) => remaining = width - 1,
            (_, 0) => remaining -= 1,
            _ => return false,
        }
    }
    remaining == 0
}

impl Renderer {
    fn sync_packed_span(&mut self, front: &RenderBuffer, y: u16, span: TouchedSpan) -> bool {
        let Some(source) = front.buffer.row(y) else {
            return false;
        };
        let Some(destination) = self.back_buf.buffer.row(y) else {
            return false;
        };
        let start = span.first as usize;
        let end = (span.last as usize + 1).min(source.len());
        if start >= end {
            return true;
        }
        let source = source.slice(start..end);
        let destination = destination.slice(start..end);
        let (CellRow::Packed(cells, resources), CellRow::Packed(previous, previous_resources)) =
            (source, destination)
        else {
            return false;
        };
        let independent = resources.is_empty() && previous_resources.is_empty()
            || cells
                .iter()
                .chain(previous)
                .all(|cell| cell.is_resource_free());
        let narrow = cells.iter().chain(previous).all(|cell| cell.width() == 1);
        if !narrow && (!complete_clusters(cells) || !complete_clusters(previous)) {
            return false;
        }
        let (mut first, mut last) = if independent {
            let Some(first) = cells.iter().zip(previous).position(|(a, b)| a != b) else {
                return true;
            };
            let last = cells
                .iter()
                .zip(previous)
                .rposition(|(a, b)| a != b)
                .unwrap()
                + 1;
            (first, last)
        } else {
            let equal =
                |a: &PackedCell, b: &PackedCell| resources.value_eq(*a, previous_resources, *b);
            let Some(first) = cells.iter().zip(previous).position(|(a, b)| !equal(a, b)) else {
                return true;
            };
            let last = cells
                .iter()
                .zip(previous)
                .rposition(|(a, b)| !equal(a, b))
                .unwrap()
                + 1;
            (first, last)
        };
        // Equal continuations still belong to their primary. Both rows are
        // complete here, so common primary boundaries close the differences.
        while cells[first].is_continuation() || previous[first].is_continuation() {
            first -= 1;
        }
        while last < cells.len()
            && (cells[last].is_continuation() || previous[last].is_continuation())
        {
            last += 1;
        }
        self.back_buf
            .buffer
            .row_mut(y)
            .unwrap()
            .slice(start + first..start + last)
            .copy_from(source.slice(first..last));
        self.back_buf
            .touch_line(y, (start + first) as u16, (start + last - 1) as u16);
        true
    }

    /// Sync a caller-facing buffer into the renderer staging buffer.
    ///
    /// # Parameters
    ///
    /// - `front`: desired cell grid whose touched spans identify writes
    ///   since the last sync.
    ///
    /// # Returns
    ///
    /// `true` when the renderer has real work to render: either the
    /// staging buffer now has touched rows that differ from its previous
    /// contents, the dimensions changed, or a force-clear is pending.
    /// Returns `false` for pure no-op writes.
    ///
    /// # Behavior
    ///
    /// Copies primary cells within `front`'s touched spans and synchronizes
    /// continuation styles separately. Clears `front`'s touched flags before
    /// returning.
    pub(crate) fn sync_front(&mut self, front: &mut RenderBuffer) -> bool {
        let width = front.width();
        let height = front.height();
        if self.back_buf.width() != width || self.back_buf.height() != height {
            self.back_buf.resize(width, height);
            // Copies below mark real cell changes; geometry changes also
            // reach the frame pipeline when every added cell is blank.
            self.back_buf.clear_touched();
        }

        for y in 0..height {
            let Some(span) = front.touched(y) else {
                continue;
            };
            if self.sync_packed_span(front, y, span) {
                continue;
            }
            for x in span.first..=span.last {
                let pos = (x, y).into();
                let Some(new_cell) = front.cell(pos) else {
                    continue;
                };
                if new_cell.is_continuation() {
                    // A primary write regenerates its continuations, so restore
                    // each column's independent style afterward.
                    if self
                        .back_buf
                        .cell(pos)
                        .is_some_and(|old| !old.style_eq_ref(new_cell))
                    {
                        self.back_buf.set_style(pos, new_cell.to_style());
                    }
                    continue;
                }
                // `RenderBuffer::set_cell` does a value-equality
                // check before writing, so unchanged cells pay no
                // clone and no touch.
                self.back_buf.set_cell(pos, new_cell);
            }
        }
        front.clear_touched();

        self.force_clear || self.back_buf.has_changes() || self.size_changed(width, height)
    }

    pub(super) fn size_changed(&self, width: u16, height: u16) -> bool {
        width != self.last_width || height != self.last_height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::renderer::buffer::RenderBuffer;

    fn cell(c: &str) -> Cell {
        Cell::new(c, 1)
    }

    fn fill_row(buf: &mut RenderBuffer, y: u16, content: &str) {
        for (x, ch) in content.chars().enumerate() {
            buf.set_cell((x as u16, y), &cell(&ch.to_string()));
        }
    }

    #[test]
    fn sync_lazy_resizes_back_buf() {
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(10, 3);
        r.sync_front(&mut front);
        assert_eq!(r.back_buf.width(), 10);
        assert_eq!(r.back_buf.height(), 3);
    }

    #[test]
    fn sync_clears_front_touched() {
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(10, 3);
        fill_row(&mut front, 0, "Hello");
        r.sync_front(&mut front);
        assert!(!front.has_changes());
    }

    #[test]
    fn sync_propagates_real_changes_to_back_buf() {
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(10, 3);
        fill_row(&mut front, 1, "World");
        let need = r.sync_front(&mut front);
        assert!(need);
        assert!(r.back_buf.touched(1).is_some());
        assert!(r.back_buf.touched(0).is_none());
        assert_eq!(r.back_buf.cell((0, 1).into()).unwrap().content(), "W");
    }

    #[test]
    fn sync_skips_touched_rows_with_identical_content() {
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(10, 3);
        fill_row(&mut front, 0, "Hello");
        fill_row(&mut front, 1, "World");
        // Render the first sync to establish the displayed geometry.
        assert!(r.sync_front(&mut front));
        r.render_back(&mut Vec::new()).unwrap();

        // Second pass: touch every row but write identical content.
        front.touch_all();
        fill_row(&mut front, 0, "Hello");
        fill_row(&mut front, 1, "World");
        let need = r.sync_front(&mut front);
        assert!(!need, "identical content must leave back_buf untouched");
        assert!(!r.back_buf.has_changes());
    }

    #[test]
    fn sync_only_touches_rows_that_actually_changed() {
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(10, 3);
        fill_row(&mut front, 0, "Hello");
        fill_row(&mut front, 1, "World");
        fill_row(&mut front, 2, "!!!!");
        r.sync_front(&mut front);
        r.back_buf.clear_touched();

        // Touch all rows but only row 1 genuinely changes.
        front.touch_all();
        fill_row(&mut front, 0, "Hello");
        fill_row(&mut front, 1, "Xorld");
        fill_row(&mut front, 2, "!!!!");
        assert!(r.sync_front(&mut front));
        assert!(r.back_buf.touched(0).is_none());
        assert!(r.back_buf.touched(1).is_some());
        assert!(r.back_buf.touched(2).is_none());
    }

    #[test]
    fn sync_only_walks_touched_spans() {
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(10, 3);
        fill_row(&mut front, 0, "preset");
        fill_row(&mut front, 2, "preset");
        r.sync_front(&mut front);
        r.back_buf.clear_touched();

        // A fresh buffer leaves blank cells untouched. Only its explicit
        // write may replace the content from the previous buffer.
        let mut front = RenderBuffer::new(10, 3);
        front.set_cell((3, 0), &cell("X"));
        r.sync_front(&mut front);
        assert!(r.back_buf.touched(0).is_some());
        assert!(r.back_buf.touched(2).is_none());
        assert_eq!(r.back_buf.cell((3, 0).into()).unwrap().content(), "X");
        assert_eq!(
            r.back_buf.cell((0, 0).into()).unwrap().content(),
            "p",
            "columns outside the touched span must keep their content"
        );
        assert_eq!(
            r.back_buf.cell((0, 2).into()).unwrap().content(),
            "p",
            "row 2 in back_buf must keep its pre-existing content"
        );
    }

    #[test]
    fn sync_reports_force_clear_even_with_no_changes() {
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(10, 3);
        assert!(r.sync_front(&mut front));
        r.render_back(&mut Vec::new()).unwrap();
        assert!(!r.sync_front(&mut front));

        r.request_clear();
        assert!(r.sync_front(&mut front));
    }

    #[test]
    fn sync_preserves_wide_primaries_and_their_continuations() {
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(10, 1);
        // Wide CJK cell at col 0; col 1 is its continuation.
        let wide = Cell::new("漢", 2);
        front.set_cell((0, 0), &wide);
        front.set_cell((3, 0), &cell("a"));
        r.sync_front(&mut front);

        assert_eq!(r.back_buf.cell((0, 0).into()).unwrap().content(), "漢");
        assert_eq!(r.back_buf.cell((0, 0).into()).unwrap().width(), 2);
        assert!(
            r.back_buf.cell((1, 0).into()).unwrap().is_continuation(),
            "col 1 must remain the wide cell's continuation, not be blanked"
        );
        assert_eq!(r.back_buf.cell((3, 0).into()).unwrap().content(), "a");
    }

    #[test]
    fn narrow_sync_matches_cell_writes_and_exact_damage() {
        use crate::color::Color;
        use crate::style::Style;

        for content in ["", "x", "▀", "\u{1f600}"] {
            for changed in [&[0][..], &[7][..], &[2, 5][..]] {
                let mut front = RenderBuffer::new(8, 1);
                let mut r = Renderer::new();
                r.back_buf.resize(8, 1);
                r.back_buf.clear_touched();
                let value = Cell::new(content, 1)
                    .style(Style::new().fg(Color::Rgb(3, 5, 7)).bg(Color::Indexed(0)));
                for &x in changed {
                    front.set_cell((x, 0), &value);
                }
                let mut reference = r.back_buf.clone();
                for x in 0..8 {
                    reference.set_cell((x, 0), front.cell((x, 0).into()).unwrap());
                }
                let span = TouchedSpan { first: 0, last: 7 };
                assert!(r.sync_packed_span(&front, 0, span));
                for x in 0..8 {
                    assert_eq!(
                        r.back_buf.cell((x, 0).into()),
                        reference.cell((x, 0).into()),
                    );
                }
                assert_eq!(
                    r.back_buf.touched(0).map(|s| (s.first, s.last)),
                    reference.touched(0).map(|s| (s.first, s.last)),
                );
                r.back_buf.clear_touched();
                assert!(r.sync_packed_span(&front, 0, span));
                assert!(r.back_buf.touched(0).is_none());
            }
        }
    }

    #[test]
    fn packed_sync_imports_selected_arena_resources() {
        use crate::style::Style;

        for value in [
            Cell::new("e\u{301}", 1),
            Cell::new("l", 1).style(Style::new().link("target", "id=sync")),
        ] {
            let mut front = RenderBuffer::new(4, 1);
            front.set_cell((1, 0), &value);
            let mut r = Renderer::new();
            r.back_buf.resize(4, 1);
            r.back_buf.clear_touched();
            assert!(r.sync_packed_span(&front, 0, TouchedSpan { first: 0, last: 3 }));
            assert!(r.back_buf.touched(0).is_some());
            r.sync_front(&mut front);
            for x in 0..4 {
                assert_eq!(r.back_buf.cell((x, 0).into()), front.cell((x, 0).into()));
            }
            front.set_cell((1, 0), &cell("x"));
            assert!(r.sync_packed_span(&front, 0, TouchedSpan { first: 1, last: 2 }));
            r.sync_front(&mut front);
            assert_eq!(r.back_buf.buffer.row(0), front.buffer.row(0));
        }
    }

    #[test]
    fn narrow_sync_preserves_destination_wide_cell_cleanup() {
        let mut front = RenderBuffer::new(4, 1);
        front.set_cell((2, 0), &cell("x"));
        let mut r = Renderer::new();
        r.back_buf.resize(4, 1);
        r.back_buf.set_cell((1, 0), &Cell::new("中", 2));
        r.back_buf.clear_touched();
        let mut reference = r.back_buf.clone();
        reference.set_cell((2, 0), &cell("x"));
        assert!(!r.sync_packed_span(&front, 0, TouchedSpan { first: 2, last: 2 }));
        r.sync_front(&mut front);
        for x in 0..4 {
            assert_eq!(
                r.back_buf.cell((x, 0).into()),
                reference.cell((x, 0).into())
            );
        }
        assert_eq!(
            r.back_buf.touched(0).map(|s| (s.first, s.last)),
            reference.touched(0).map(|s| (s.first, s.last)),
        );
    }

    #[test]
    fn packed_sync_uses_complete_wide_spans_with_resources_elsewhere() {
        use crate::color::Color;
        use crate::style::Style;

        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(8, 2);
        let rich =
            Cell::new("e\u{301}", 1).style(Style::new().link("https://example.test/sync", ""));
        front.set_cell((0, 0), &rich);
        front.set_cell((7, 1), &rich);
        front.set_cell((2, 0), &Cell::new("界", 2));
        front.set_cell((5, 0), &cell("a"));
        r.sync_front(&mut front);
        r.render_back(&mut Vec::new()).unwrap();

        for glyph in ["漢", "語"] {
            front.set_cell((2, 0), &Cell::new(glyph, 2));
            front.set_style((3, 0), Style::new().bg(Color::Indexed(4)));
            front.set_cell((5, 0), &cell("b"));
            assert!(r.sync_packed_span(&front, 0, TouchedSpan { first: 2, last: 5 }));
            assert_eq!(r.back_buf.buffer.row(0), front.buffer.row(0));
            let span = r.back_buf.touched(0).unwrap();
            assert!(span.first <= 2 && span.last >= 3);
            r.back_buf.clear_touched();
        }

        // Only the primary differs; an equal continuation still belongs to it.
        front.set_style((2, 0), Style::new().fg(Color::Rgb(3, 5, 7)));
        assert!(r.sync_packed_span(&front, 0, TouchedSpan { first: 2, last: 3 }));
        assert_eq!(r.back_buf.buffer.row(0), front.buffer.row(0));
        assert_eq!(r.back_buf.cell((2, 0).into()).unwrap().width(), 2);
        assert_eq!(r.back_buf.touched(0).unwrap().last, 3);
        r.back_buf.clear_touched();
        assert!(r.sync_packed_span(&front, 0, TouchedSpan { first: 2, last: 3 }));
        assert!(!r.back_buf.has_changes());
    }

    #[test]
    fn packed_sync_rejects_partial_source_and_destination_clusters() {
        for source_wide in [false, true] {
            let mut r = Renderer::new();
            let mut front = RenderBuffer::new(6, 1);
            r.back_buf.resize(6, 1);
            if source_wide {
                front.set_cell((2, 0), &Cell::new("界", 2));
            } else {
                r.back_buf.set_cell((2, 0), &Cell::new("界", 2));
            }
            r.back_buf.clear_touched();
            for span in [
                TouchedSpan { first: 2, last: 2 },
                TouchedSpan { first: 3, last: 4 },
            ] {
                assert!(!r.sync_packed_span(&front, 0, span));
                assert!(!r.back_buf.has_changes());
            }
            front.touch_full_line(0);
            assert!(r.sync_packed_span(&front, 0, front.touched(0).unwrap()));
            assert_eq!(r.back_buf.buffer.row(0), front.buffer.row(0));
        }
    }

    #[test]
    fn sync_normalizes_a_cluster_split_by_shrink_then_grow() {
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(6, 1);
        front.set_cell((1, 0), &Cell::new("界", 4));
        r.sync_front(&mut front);
        r.render_back(&mut Vec::new()).unwrap();

        front.resize(3, 1);
        front.resize(6, 1);
        let mut expected = r.back_buf.clone();
        for x in 0..6 {
            expected.set_cell((x, 0), front.cell((x, 0).into()).unwrap());
        }
        r.sync_front(&mut front);
        assert_eq!(r.back_buf.buffer.row(0), expected.buffer.row(0));
        assert!(r.back_buf.cell((1, 0).into()).unwrap().is_blank());
    }
}

//! Front → back buffer sync with per-cell equality filtering.
//!
//! Screens write freely into their `front_buf`; `fill_rect` and bulk
//! ops mark whole rows as touched even when the resulting content
//! matches the previous frame. [`Renderer::sync_front`] copies those
//! touched spans into [`Renderer::back_buf`]. Complete clusters use
//! owned slice copies; partial clusters use [`RenderBuffer::set_cell`].
//! Both paths mark only changed spans. The diff pass then
//! visits only rows that genuinely changed on the wire.
//!
//! Only the cells inside `front_buf.touched(y)` spans are read — no
//! full-buffer scan.

use crate::renderer::Renderer;
use crate::renderer::buffer::{RenderBuffer, TouchedSpan};

use crate::buffer::Surface;
use crate::cell::Cell;

fn complete_clusters(cells: &[Cell]) -> bool {
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
    fn sync_owned_span(&mut self, front: &RenderBuffer, y: u16, span: TouchedSpan) -> bool {
        let start = usize::from(span.first);
        let end = (usize::from(span.last) + 1).min(usize::from(front.width()));
        if start >= end {
            return true;
        }
        let source = &front.line(y).unwrap()[start..end];
        let previous = &self.back_buf.line(y).unwrap()[start..end];
        if !complete_clusters(source) || !complete_clusters(previous) {
            return false;
        }
        let Some(mut first) = source.iter().zip(previous).position(|(a, b)| a != b) else {
            return true;
        };
        let mut last = source
            .iter()
            .zip(previous)
            .rposition(|(a, b)| a != b)
            .unwrap()
            + 1;
        // Close both ends at primary boundaries in both complete rows.
        while source[first].is_continuation() || previous[first].is_continuation() {
            first -= 1;
        }
        while last < source.len()
            && (source[last].is_continuation() || previous[last].is_continuation())
        {
            last += 1;
        }
        self.back_buf.line_mut(y).unwrap()[start + first..start + last]
            .clone_from_slice(&source[first..last]);
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
    /// Copies primary cells within `front`'s touched spans and restores
    /// independent continuation styles. Clears `front`'s touched flags.
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
            if self.sync_owned_span(front, y, span) {
                continue;
            }
            for x in span.first..=span.last {
                let pos = (x, y).into();
                let Some(new_cell) = front.cell(pos) else {
                    continue;
                };
                if new_cell.is_continuation() {
                    // A primary write regenerates its continuations.
                    if self.back_buf.cell(pos).is_some_and(|old| {
                        old.fg != new_cell.fg
                            || old.bg != new_cell.bg
                            || old.underline_color != new_cell.underline_color
                            || old.attrs != new_cell.attrs
                            || old.underline != new_cell.underline
                            || old.link != new_cell.link
                    }) {
                        let old = self.back_buf.cell_mut(pos).unwrap();
                        old.fg = new_cell.fg;
                        old.bg = new_cell.bg;
                        old.underline_color = new_cell.underline_color;
                        old.attrs = new_cell.attrs;
                        old.underline = new_cell.underline;
                        old.link.clone_from(&new_cell.link);
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
    use crate::style::Style;

    fn cell(c: &str) -> Cell {
        Cell::new(c, 1, Style::EMPTY)
    }

    fn fill_row(buf: &mut RenderBuffer, y: u16, content: &str) {
        for (x, ch) in content.chars().enumerate() {
            buf.set_cell((x as u16, y), &cell(&ch.to_string()));
        }
    }

    #[test]
    fn sync_validates_clusters_inside_both_spans_after_resize() {
        for broken_front in [false, true] {
            let mut r = Renderer::new();
            let mut front = RenderBuffer::new(8, 1);
            r.sync_front(&mut front);
            let broken = if broken_front {
                &mut front
            } else {
                &mut r.back_buf
            };
            broken.set_cell((1, 0), &Cell::new("wide", 4, Style::EMPTY));
            broken.resize(3, 1);
            broken.resize(8, 1);
            assert_eq!(broken.cell((1, 0).into()).unwrap().width(), 4);
            assert!(!broken.cell((3, 0).into()).unwrap().is_continuation());
            front.set_cell((0, 0), &cell("L"));
            front.set_cell((7, 0), &cell("R"));
            front.touch_all();

            let mut expected = r.back_buf.buffer.clone();
            for (x, cell) in front.line(0).unwrap().iter().enumerate() {
                let pos = (x as u16, 0);
                if cell.is_continuation() {
                    expected
                        .cell_mut(pos.into())
                        .unwrap()
                        .set_style(cell.style());
                } else {
                    expected.set(pos, cell);
                }
            }
            r.sync_front(&mut front);
            assert_eq!(r.back_buf.line(0), expected.line(0), "front={broken_front}");
        }
    }

    #[test]
    fn sync_owned_span_keeps_text_and_links_after_source_changes() {
        let text = format!("e{}", "\u{301}".repeat(40));
        let mut r = Renderer::new();
        {
            let mut front = RenderBuffer::new(4, 1);
            let source = Cell::new(
                text.as_str(),
                1,
                crate::style::Style::new()
                    .fg(crate::color::Color::Green)
                    .link("https://example.test", "id=owned"),
            );
            front.set_cell((0, 0), &source);
            r.sync_front(&mut front);
            front.set_cell((0, 0), &Cell::BLANK);
        }
        let copied = r.back_buf.cell((0, 0).into()).unwrap();
        assert_eq!(copied.content(), text);
        assert_eq!(copied.fg, Some(crate::color::Color::Green));
        assert_eq!(copied.link.as_deref().unwrap().params, "id=owned");
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

    /// Regression: sync_front used to step by 1 column even on wide
    /// cells, which re-visited the continuation column and overwrote
    /// the wide primary in back_buf with a blank. With the width-
    /// aware step the primary survives and the continuation is
    /// skipped.
    #[test]
    fn sync_advances_by_wide_cell_width_and_skips_continuation() {
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(10, 1);
        // Wide CJK cell at col 0; col 1 is its continuation.
        let wide = Cell::new("漢", 2, Style::EMPTY);
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
}

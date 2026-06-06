//! Front → back buffer sync with per-cell equality filtering.
//!
//! Screens write freely into their `front_buf`; `fill_rect` and bulk
//! ops mark whole rows as touched even when the resulting content
//! matches the previous frame. [`Renderer::sync_front`] copies those
//! touched spans into [`Renderer::back_buf`] using
//! [`RenderBuffer::set_cell`], which only flips `back_buf.touched`
//! when the destination cell actually differs. The diff pass then
//! visits only rows that genuinely changed on the wire.
//!
//! Only the cells inside `front_buf.touched(y)` spans are read — no
//! full-buffer scan.

use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;

use crate::buffer::Surface;

impl Renderer {
    /// Sync `front` into `back_buf`, then clear `front`'s touched
    /// flags. Returns `true` when the resulting `back_buf` has any
    /// rows the diff pipeline must redraw (or when a force-clear is
    /// pending).
    pub(crate) fn sync_front(&mut self, front: &mut RenderBuffer) -> bool {
        let width = front.width();
        let height = front.height();
        if self.back_buf.width() != width || self.back_buf.height() != height {
            self.back_buf.resize(width, height);
            // `resize` touches every row as a bookkeeping side effect.
            // We use `touched` to mean "differs from the wire", so
            // reset it; cell-by-cell copies below will re-touch only
            // rows that actually changed. The caller is expected to
            // have set `force_clear` (e.g. via `request_clear` from
            // [`crate::screen::Screen::resize`]) to drive a full
            // repaint when dimensions change.
            self.back_buf.clear_touched();
        }

        for y in 0..height {
            let Some(span) = front.touched(y) else {
                continue;
            };
            let mut x = span.first;
            while x <= span.last {
                let pos = (x, y).into();
                let Some(new_cell) = front.cell(pos) else {
                    x += 1;
                    continue;
                };
                // Skip continuation columns: their owning primary
                // (written one or more columns to the left) already
                // populated them via [`Buffer::set`]'s wide-cell
                // bookkeeping. Visiting them directly would either be a
                // no-op (continuation→continuation) or, worse, blank
                // the primary we just mirrored.
                if new_cell.is_continuation() {
                    x += 1;
                    continue;
                }
                // `RenderBuffer::set_cell` does a reference-equality
                // check before writing, so unchanged cells pay no
                // clone and no touch.
                self.back_buf.set_cell(pos, new_cell);
                // Step over any continuation columns owned by the cell
                // we just wrote so we don't try to re-copy them.
                let step = (new_cell.width()).max(1);
                x = x.saturating_add(step);
            }
        }
        front.clear_touched();

        self.force_clear || self.back_buf.has_changes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::renderer::buffer::RenderBuffer;

    fn cell(c: &str) -> Cell {
        Cell::narrow(c)
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
        // First sync: back_buf gets the content + touched.
        assert!(r.sync_front(&mut front));
        r.back_buf.clear_touched();

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
        // Writing a single cell must not propagate other rows or
        // columns to back_buf — verified by leaving back_buf in a
        // distinct state outside the span and confirming it survives.
        let mut r = Renderer::new();
        let mut front = RenderBuffer::new(10, 3);
        // Pre-seed back_buf row 2 with content via a full sync.
        fill_row(&mut front, 2, "preset");
        r.sync_front(&mut front);
        r.back_buf.clear_touched();

        // Now write one cell on row 0; row 2 of front is blank-but-
        // untouched, so sync must not visit row 2.
        front.set_cell((3, 0), &cell("X"));
        r.sync_front(&mut front);
        assert!(r.back_buf.touched(0).is_some());
        assert!(r.back_buf.touched(2).is_none());
        assert_eq!(
            r.back_buf.cell((0, 2).into()).unwrap().content(),
            "p",
            "row 2 in back_buf must keep its pre-existing content"
        );
    }

    #[test]
    fn sync_reports_force_clear_even_with_no_changes() {
        let mut r = Renderer::new();
        r.force_clear = true;
        let mut front = RenderBuffer::new(10, 3);
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
        let wide = Cell::wide("漢");
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

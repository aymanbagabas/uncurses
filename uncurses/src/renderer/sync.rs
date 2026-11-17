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
//! Only cells inside touched spans participate in synchronization.

use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;

use crate::buffer::Surface;

impl Renderer {
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
    /// Copies only cells within `front`'s touched spans, skips
    /// continuation cells, steps by wide-cell width, and clears
    /// `front`'s touched flags before returning.
    pub(crate) fn sync_front(&mut self, front: &mut RenderBuffer) -> bool {
        self.sync_front_impl::<true>(front)
            .expect("placement metadata is accepted")
    }

    pub(crate) fn sync_front_text(&mut self, front: &mut RenderBuffer) -> std::io::Result<bool> {
        self.sync_front_impl::<false>(front)
    }

    fn sync_front_impl<const PLACEMENTS: bool>(
        &mut self,
        front: &mut RenderBuffer,
    ) -> std::io::Result<bool> {
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
            let mut x = span.first;
            while x <= span.last {
                let pos = (x, y).into();
                let Some(new_cell) = front.cell(pos) else {
                    x += 1;
                    continue;
                };
                // Skip continuation columns. The primary write already copies them.
                if new_cell.is_continuation() {
                    x += 1;
                    continue;
                }
                let existing = self.back_buf.cell(pos).unwrap();
                let previous_width = existing.width();
                let changed = existing != new_cell || u16::from(new_cell.width()) > width - x;
                if !changed {
                    x = x.saturating_add(u16::from(new_cell.width()));
                    continue;
                }
                if new_cell.is_placement() {
                    if !PLACEMENTS {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "unregistered placement cell",
                        ));
                    }
                    self.back_buf
                        .set_cell(pos, &crate::cell::Cell::BLANK.style(&new_cell.style));
                } else {
                    self.back_buf.replace_cell(pos, new_cell, previous_width);
                }
                // Step over any continuation columns owned by the cell
                // we just wrote so we don't try to re-copy them.
                let step = (new_cell.width() as u16).max(1);
                x = x.saturating_add(step);
            }
        }
        front.clear_touched();

        Ok(self.force_clear || self.back_buf.has_changes() || self.size_changed(width, height))
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
    fn sync_placement_preserves_style_in_backing_blank() {
        let mut renderer = Renderer::new();
        let mut front = RenderBuffer::new(2, 1);
        let style = crate::style::Style::new()
            .bold()
            .bg(crate::color::Color::Blue)
            .link("https://example.com", "");
        front.set_cell((0, 0), &Cell::placement(1).style(&style));
        renderer.sync_front(&mut front);
        assert_eq!(
            renderer.back_buf.cell((0, 0).into()),
            Some(&Cell::BLANK.style(style))
        );
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
    fn sync_normalizes_an_equal_wide_primary_clipped_by_resize() {
        let mut renderer = Renderer::new();
        let mut front = RenderBuffer::new(3, 1);
        let style = crate::style::Style::new().bold();
        front.set_cell((1, 0), &Cell::new("\u{754c}", 2).style(&style));
        renderer.sync_front(&mut front);

        front.resize(2, 1);
        renderer.sync_front(&mut front);
        assert_eq!(
            renderer.back_buf.cell((1, 0).into()),
            Some(&Cell::BLANK.style(style))
        );
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
}

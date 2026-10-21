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
use crate::renderer::regions::Coverage;

use crate::buffer::Surface;
use crate::cell::Cell;
#[cfg(any(test, uncurses_bench))]
use crate::layout::Rect;

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
    /// contents, or a force-clear is pending. Returns `false` for pure
    /// no-op writes.
    ///
    /// # Behavior
    ///
    /// Copies only cells within `front`'s touched spans, skips
    /// continuation cells, steps by wide-cell width, and clears
    /// `front`'s touched flags before returning.
    pub(crate) fn sync_front(&mut self, front: &mut RenderBuffer) -> bool {
        self.sync_front_covered(front, &Coverage::default())
    }

    /// Project covered glyphs as styled spaces into the existing staging buffer.
    /// The source keeps its cells, including text beneath regions.
    /// Region lookups advance by covered runs rather than by individual cells.
    pub(crate) fn sync_front_covered(
        &mut self,
        front: &mut RenderBuffer,
        coverage: &Coverage,
    ) -> bool {
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
            if self.cur_buf.is_some() {
                self.request_clear();
            }
        }

        for y in 0..height {
            let Some(span) = front.touched(y) else {
                continue;
            };
            let covered = coverage.row(y);
            let mut index = covered.partition_point(|r| r.end <= span.first);
            let mut x = span.first;
            while x <= span.last {
                let pos = (x, y).into();
                let Some(new_cell) = front.cell(pos) else {
                    x += 1;
                    continue;
                };
                // Skip continuation columns. Copying the primary places
                // the continuation on the far side as part of the same
                // write, so the column is already carrying the value this
                // loop would ask for. `Buffer::set` ignores a continuation
                // offered on its own, so passing one on would be a wasted
                // call rather than a wrong one, and stepping over it keeps
                // the walk in step with the widths it advances by.
                if new_cell.is_continuation() {
                    x += 1;
                    continue;
                }
                // `RenderBuffer::set_cell` does a value-equality
                // check before writing, so unchanged cells pay no
                // clone and no touch.
                let step = u16::from(new_cell.width().max(1));
                while covered.get(index).is_some_and(|r| x >= r.end) {
                    index += 1;
                }
                if covered
                    .get(index)
                    .is_some_and(|r| r.start < x.saturating_add(step))
                {
                    // A glyph cannot be partially visible behind a region.
                    let blank = Cell::BLANK.style(new_cell.style.clone());
                    for col in x..x.saturating_add(step).min(width) {
                        self.back_buf.set_cell((col, y), &blank);
                    }
                } else {
                    self.back_buf.set_cell(pos, new_cell);
                }
                // Step over any continuation columns owned by the cell
                // we just wrote so we don't try to re-copy them.
                x = x.saturating_add(step);
            }
        }
        front.clear_touched();

        self.force_clear || self.back_buf.has_changes()
    }
}

#[cfg(uncurses_bench)]
mod bench {
    extern crate test;

    use super::*;
    use test::{Bencher, black_box};

    fn sync_touched(b: &mut Bencher, rects: impl Iterator<Item = Rect> + Clone) {
        let coverage = Coverage::from_rects(rects);
        let mut renderer = Renderer::new();
        let mut front = RenderBuffer::new(160, 48);
        for y in 0..48 {
            for x in 0..160 {
                front.set_cell((x, y), &Cell::new("x", 1));
            }
        }
        renderer.sync_front_covered(&mut front, &coverage);
        renderer.back_buf.clear_touched();
        b.iter(|| {
            front.touch_all();
            black_box(renderer.sync_front_covered(black_box(&mut front), black_box(&coverage)));
        });
    }

    #[bench]
    fn text_only(b: &mut Bencher) {
        sync_touched(b, std::iter::empty());
    }

    #[bench]
    fn one_region(b: &mut Bencher) {
        sync_touched(b, [Rect::new(4, 2, 8, 2)].into_iter());
    }

    #[bench]
    fn many_regions(b: &mut Bencher) {
        let rects: std::collections::BTreeMap<_, _> = (0..64)
            .map(|i| (i, Rect::new((i % 8) * 20 + 4, (i / 8) * 6 + 2, 8, 2)))
            .collect();
        sync_touched(b, rects.values().copied());
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

    #[test]
    fn regions_project_whole_wide_glyphs_without_changing_source_cells() {
        let mut renderer = Renderer::new();
        let mut front = RenderBuffer::new(10, 2);
        front.set_cell((1, 0), &Cell::new("\u{754c}", 2));
        front.set_cell((5, 0), &cell("X"));
        let covered = [Rect::new(2, 0, 4, 1)];
        renderer.sync_front_covered(&mut front, &Coverage::from_rects(covered.into_iter()));
        assert_eq!(front.cell((1, 0).into()).unwrap().content(), "\u{754c}");
        assert_eq!(front.cell((5, 0).into()).unwrap().content(), "X");
        for x in 1..6 {
            assert!(renderer.back_buf.cell((x, 0).into()).unwrap().is_blank());
        }
        front.touch_full_line(0);
        renderer.sync_front(&mut front);
        assert_eq!(renderer.back_buf.line(0), front.line(0));
    }

    #[test]
    fn region_split_spaces_preserve_the_complete_glyph_style() {
        use crate::color::Color;
        use crate::style::Style;

        let style = Style::default()
            .fg(Color::BrightYellow)
            .bg(Color::Blue)
            .bold()
            .faint()
            .italic()
            .link("https://example.com", "id=split");
        let glyph = Cell::new("\u{754c}", 2).style(style.clone());
        for covered in [Rect::new(1, 0, 1, 1), Rect::new(2, 0, 1, 1)] {
            let mut renderer = Renderer::new();
            let mut front = RenderBuffer::new(5, 1);
            front.set_cell((1, 0), &glyph);
            let coverage = Coverage::from_rects([covered].into_iter());
            renderer.sync_front_covered(&mut front, &coverage);
            for x in 1..3 {
                assert_eq!(
                    renderer.back_buf.cell((x, 0).into()),
                    Some(&Cell::BLANK.style(style.clone()))
                );
            }
            assert_eq!(front.cell((1, 0).into()), Some(&glyph));
            let mut out = Vec::new();
            renderer.set_region_rects([covered].into_iter());
            renderer.render_back(&mut out).unwrap();
            renderer.region_drawn(covered);
            assert_eq!(
                renderer.cur_buf.as_ref().unwrap().line(0),
                renderer.back_buf.line(0)
            );
            front.touch_all();
            assert!(!renderer.sync_front_covered(&mut front, &coverage));
            front.touch_all();
            renderer.sync_front(&mut front);
            assert_eq!(renderer.back_buf.cell((1, 0).into()), Some(&glyph));
        }
    }

    #[test]
    fn region_projection_reuses_cached_row_coverage() {
        let mut renderer = Renderer::new();
        let mut front = RenderBuffer::new(160, 3);
        front.touch_all();
        let visits = std::cell::Cell::new(0);
        let rects = [Rect::new(16, 1, 96, 1)]
            .into_iter()
            .inspect(|_| visits.set(visits.get() + 1));
        let coverage = Coverage::from_rects(rects);
        for _ in 0..3 {
            front.touch_all();
            renderer.sync_front_covered(&mut front, &coverage);
        }
        assert_eq!(visits.get(), 1);
    }

    #[test]
    fn region_projection_handles_unordered_rectangles_and_wide_boundaries() {
        let cases = [
            vec![],
            vec![
                Rect::new(8, 0, 2, 1),
                Rect::new(2, 0, 2, 2),
                Rect::new(4, 0, 2, 1),
            ],
            vec![Rect::new(4, 0, 7, 1), Rect::new(2, 0, 2, 2)],
            vec![Rect::new(2, 0, 0, 2), Rect::new(1, 0, 4, 0)],
        ];
        for rects in cases {
            let mut front = RenderBuffer::new(14, 2);
            fill_row(&mut front, 0, "abcdefghijklmn");
            fill_row(&mut front, 1, "ABCDEFGHIJKLMN");
            for x in [1, 5, 7] {
                front.set_cell((x, 0), &Cell::new("\u{754c}", 2));
            }
            let source = front.clone();
            let mut expected = RenderBuffer::new(14, 2);
            for y in 0..2 {
                for x in 0..14 {
                    let cell = source.cell((x, y).into()).unwrap();
                    if cell.is_continuation() {
                        continue;
                    }
                    let footprint = Rect::new(x, y, u16::from(cell.width().max(1)), 1);
                    if !rects.iter().any(|r| !r.intersection(footprint).is_empty()) {
                        expected.set_cell((x, y), cell);
                    }
                }
            }
            let mut renderer = Renderer::new();
            let coverage = Coverage::from_rects(rects.iter().copied());
            renderer.sync_front_covered(&mut front, &coverage);
            for y in 0..2 {
                assert_eq!(renderer.back_buf.line(y), expected.line(y), "{rects:?}");
                assert_eq!(front.line(y), source.line(y));
            }
            renderer.back_buf.clear_touched();
            front.touch_all();
            assert!(!renderer.sync_front_covered(&mut front, &coverage));
        }
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

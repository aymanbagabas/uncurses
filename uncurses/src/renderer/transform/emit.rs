//! Cell emission helpers: `emit_range` (rangewise dispatch among
//! plain/ECH/REP), `emit_cell` (single grapheme), `put_range`
//! (consecutive run), and `insert_cells_op` (ICH).

use std::io;

use super::predicates::{can_clear_with, cells_eq_diff, is_rep_ascii};
use crate::ansi;
use crate::cell::Cell;
use crate::renderer::caps::Optimizations;
use crate::renderer::{RenderBuffer, Renderer};

impl Renderer {
    /// Emit a range of cells from a line, handling style transitions.
    ///
    /// Returns `true` when the emission left the cursor partway through
    /// the requested interval and the rest of it is already correct on
    /// screen (currently: ECH covering the trailing blank run). The
    /// caller must move the cursor past the interval before issuing any
    /// follow-up positional operation; otherwise it can return without
    /// a move because the cursor naturally landed past the end of the
    /// interval.
    ///
    /// The body uses a unified equal-run grouping: leading singletons
    /// are emitted one-by-one, then any run of cells equal to the run's
    /// first cell dispatches to ECH (when the cell is erasable), REP
    /// (single ASCII byte), or a plain per-cell emission depending on
    /// which option is cheapest.
    pub(super) fn emit_range(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &RenderBuffer,
        line: &[Cell],
        first: usize,
        last: usize,
    ) -> io::Result<bool> {
        let surface_width = self.last_width;
        let surface_height = self.last_height;
        let has_ech = self.opts.contains(Optimizations::ECH);
        let has_rep = self.opts.contains(Optimizations::REP);

        // When neither optimization is supported, fall through to a
        // simple per-cell emission with no equality scanning.
        if !has_ech && !has_rep {
            let mut x = first;
            while x <= last {
                let cell = &line[x];
                if cell.is_continuation() {
                    x += 1;
                    continue;
                }
                self.emit_cell(out, cell, surface_width, surface_height)?;
                x += cell.width().max(1) as usize;
            }
            return Ok(false);
        }

        let mut x = first;
        while x <= last {
            // Continuation cells of a wide character produce no output
            // and never participate in an equal-run since they don't
            // compare equal to the cell that produced them.
            if line[x].is_continuation() {
                x += 1;
                continue;
            }

            let stride = line[x].width().max(1) as usize;
            let next_idx = x + stride;

            // Last cell in the interval: emit and finish.
            if next_idx > last {
                self.emit_cell(out, &line[x], surface_width, surface_height)?;
                return Ok(false);
            }

            // Singleton: the cell differs from its successor.
            if line[x] != line[next_idx] {
                self.emit_cell(out, &line[x], surface_width, surface_height)?;
                x = next_idx;
                continue;
            }

            // Equal-run starting at x.
            let cell0 = &line[x];
            let mut count = 2u16;
            let mut j = next_idx + stride;
            while j <= last && line[j] == *cell0 {
                count += 1;
                j += stride;
            }

            let ech_b = ansi::cost::ech_cost(count);
            let cup_b = ansi::cost::cup_cost(self.cur.pos.y, self.cur.pos.x.saturating_add(count));
            let rep_b = ansi::cost::rep_cost(count);

            if has_ech
                && (count as usize) > ech_b + cup_b
                && can_clear_with(cell0, self.opts.contains(Optimizations::BCE))
            {
                self.update_pen(out, Some(cell0))?;
                ansi::write_ech(out, count)?;
                if j > last {
                    return Ok(true);
                }
                self.move_to(
                    out,
                    new_buf,
                    self.cur.pos.y,
                    self.cur.pos.x.saturating_add(count),
                )?;
                x = j;
            } else if has_rep
                && (count as usize) > rep_b
                && is_rep_ascii(cell0.content().as_bytes())
            {
                // Right-margin wrap: when the run would cross the
                // right edge, REP would lose the wrapped cell to the
                // terminal's autowrap handling, so emit one extra
                // cell at the end and shorten the REP count by one.
                let wrap_possible = self.cur.pos.x.saturating_add(count) >= surface_width;
                let mut rep_count = count;
                if wrap_possible {
                    rep_count -= 1;
                }

                self.update_pen(out, Some(cell0))?;
                self.put_glyph_bytes(
                    out,
                    cell0.content().as_bytes(),
                    1,
                    surface_width,
                    surface_height,
                )?;
                rep_count -= 1;

                if rep_count > 0 {
                    ansi::write_rep(out, rep_count)?;
                    self.cur.pos.x = self.cur.pos.x.saturating_add(rep_count).min(surface_width);
                    if self.cur.pos.x >= surface_width {
                        self.cur.at_phantom = true;
                    }
                }

                if wrap_possible {
                    self.put_glyph_bytes(
                        out,
                        cell0.content().as_bytes(),
                        1,
                        surface_width,
                        surface_height,
                    )?;
                }
                x = j;
            } else {
                // Plain emit: every cell in an equal-run shares cell0's
                // pen and content, so set the pen once and re-emit the
                // glyph bytes. Equal-run detection already excluded
                // continuation cells, so cell0 is always a printable
                // primary cell here.
                //
                // Placeholder cells render as a literal space here so
                // any prior glyph at the column is cleared; the
                // external paint layer redraws over the space on the
                // same frame.
                self.update_pen(out, Some(cell0))?;
                let (bytes, glyph_width) = if cell0.content().is_empty() {
                    (b" ".as_slice(), 1u16)
                } else {
                    (cell0.content().as_bytes(), cell0.width() as u16)
                };
                for _ in 0..count {
                    self.put_glyph_bytes(out, bytes, glyph_width, surface_width, surface_height)?;
                }
                x = j;
            }
        }

        Ok(false)
    }
    /// Emit one cell to the output, substituting a literal space for
    /// blank cells with no glyph content so the cursor still advances.
    /// Placeholder cells render as a literal space — the external
    /// paint layer redraws over the space on the same frame, so
    /// prior glyph content at the column is cleared.
    pub(super) fn emit_cell(
        &mut self,
        out: &mut Vec<u8>,
        cell: &Cell,
        surface_width: u16,
        surface_height: u16,
    ) -> io::Result<()> {
        if cell.is_continuation() {
            return Ok(());
        }
        self.update_pen(out, Some(cell))?;
        if cell.content().is_empty() {
            self.put_glyph_bytes(out, b" ", 1, surface_width, surface_height)
        } else {
            self.put_glyph_bytes(
                out,
                cell.content().as_bytes(),
                cell.width() as u16,
                surface_width,
                surface_height,
            )
        }
    }
    /// Emit cells in `new_line[start..=end]`, looking for runs of cells
    /// that already match the old line and skipping over them with a
    /// cursor move when the saved bytes outweigh the cost of the move.
    ///
    /// Returns `true` when the cursor ended up partway through the
    /// interval and the rest is already correct on screen (either a
    /// trailing matched run was skipped, or the final `emit_range`
    /// covered its tail with an ECH). The caller must move the cursor
    /// to the end of the interval before any follow-on positional
    /// operation; otherwise the cursor naturally landed at `end + 1`
    /// and no move is required.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn put_range(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &RenderBuffer,
        old_line: Option<&[Cell]>,
        new_line: &[Cell],
        y: u16,
        start: usize,
        end: usize,
    ) -> io::Result<bool> {
        // Lower bound on the cost of any in-place cursor positioning
        // available on this surface: the smallest number of bytes the
        // move planner could possibly spend to relocate the cursor
        // without writing cell content. CUP and CUF are baseline-ANSI
        // and always available; HPA and CHA are opt-gated and only
        // contribute when the planner would actually consider them.
        // Used both as the outer "is the span worth scanning?" guard
        // and as the per-iteration "is this matching run worth a
        // cursor jump?" threshold.
        let inline = {
            let s = start as u16;
            let mut c = crate::ansi::cost::cup_cost(y, s).min(crate::ansi::cost::cuf_cost(s));
            if self.opts.contains(Optimizations::HPA) {
                c = c.min(crate::ansi::cost::hpa_cost(s));
            }
            if self.opts.contains(Optimizations::CHA) {
                c = c.min(crate::ansi::cost::cha_cost(s));
            }
            c
        };
        let span = end - start + 1;
        if span > inline {
            let mut j = start;
            let mut same = 0usize;
            let mut seg_start = start;
            while j <= end {
                let old_cell = old_line.and_then(|l| l.get(j));
                let new_cell = &new_line[j];
                if same == 0
                    && old_cell.is_some_and(|o| o.is_continuation())
                    && new_cell.is_continuation()
                {
                    j += 1;
                    continue;
                }
                let equal = old_cell.is_some_and(|o| cells_eq_diff(o, new_cell));
                if equal {
                    same += 1;
                } else {
                    if same > inline {
                        // Run of matching cells exceeded the cost of an
                        // in-place cursor move; emit what we have, then
                        // skip past the run with a positioning jump.
                        let prev_end = j.saturating_sub(same).saturating_sub(1);
                        if prev_end >= seg_start {
                            self.emit_range(out, new_buf, new_line, seg_start, prev_end)?;
                        }
                        self.move_to(out, new_buf, y, j as u16)?;
                        seg_start = j;
                    }
                    same = 0;
                }
                j += 1;
            }
            // Use signed arithmetic so an all-matching segment (where
            // `same` equals the segment length) yields a negative
            // `tail_end` and the emit is skipped (a count-of-zero
            // no-op). usize saturation would clamp to 0 and spuriously
            // emit cell 0 when `seg_start == 0`.
            let tail_end = j as isize - same as isize - 1;
            let tail_eoi = if tail_end >= seg_start as isize {
                self.emit_range(out, new_buf, new_line, seg_start, tail_end as usize)?
            } else {
                false
            };
            // A skipped trailing run leaves the cursor mid-interval at
            // the last non-matching emission; the rest of the interval
            // is already correct so we report eoi to the caller.
            // Otherwise propagate whatever the tail emission reported
            // (ECH may have ended the interval the same way).
            Ok(if same != 0 { true } else { tail_eoi })
        } else {
            self.emit_range(out, new_buf, new_line, start, end)
        }
    }
    /// Insert `count` slots from the front of `line` at the current
    /// cursor position. Emits ICH when the terminal supports it,
    /// otherwise brackets the writes with IRM (set/reset insert mode)
    /// so existing content is pushed right rather than overwritten.
    ///
    /// `count` counts slots (columns), not glyphs: continuation cells
    /// of a wide character no-op but still consume one of the count
    /// units, mirroring the cursor advance the terminal performs for
    /// the inserted blanks.
    pub(super) fn insert_cells_op(
        &mut self,
        out: &mut Vec<u8>,
        line: &[Cell],
        count: usize,
    ) -> io::Result<()> {
        if count == 0 {
            return Ok(());
        }
        let use_ich = self.opts.contains(Optimizations::ICH);
        if use_ich {
            ansi::write_ich(out, count as u16)?;
        } else {
            ansi::write_set_mode(out, &[ansi::Mode::INSERT])?;
        }
        let surface_width = self.last_width;
        let surface_height = self.last_height;
        for i in 0..count {
            match line.get(i) {
                Some(cell) if cell.is_continuation() => continue,
                Some(cell) => {
                    self.update_pen(out, Some(cell))?;
                    self.put_glyph_bytes(
                        out,
                        cell.content().as_bytes(),
                        cell.width() as u16,
                        surface_width,
                        surface_height,
                    )?;
                }
                None => {
                    // Slice ran out before count; emit a literal space
                    // with a reset pen for the missing indices so the
                    // inserted region remains a uniform count slots
                    // regardless of the slice length.
                    self.reset_pen(out)?;
                    self.put_glyph_bytes(out, b" ", 1, surface_width, surface_height)?;
                }
            }
        }
        if !use_ich {
            ansi::write_reset_mode(out, &[ansi::Mode::INSERT])?;
        }
        Ok(())
    }
}

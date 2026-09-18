//! Cell emission helpers: `emit_range` (rangewise dispatch among
//! plain/ECH/REP), `emit_cell` (single grapheme), `put_range`
//! (consecutive run), and `insert_cells_op` (ICH).

use std::io;

use super::predicates::{can_clear_with, is_rep_ascii};
use crate::ansi;
use crate::cell::Cell;
use crate::renderer::caps::Optimizations;
use crate::renderer::{RenderBuffer, Renderer};

/// The first column at or before `at` that owns the cell it holds.
///
/// A diff finds its boundaries by comparing columns, so one can land inside
/// a cluster. A terminal draws a glyph or does not, so emission has to begin
/// on a whole one: the cursor is placed at a run's first column, and a
/// continuation writes no bytes and moves no cursor, which would leave every
/// glyph after it a column to the left of where it belongs.
pub(super) fn cluster_start(line: &[Cell], at: usize) -> usize {
    let here = at.min(line.len().saturating_sub(1));
    let mut start = here;
    while start > 0 && line[start].is_continuation() {
        start -= 1;
    }
    // Only a wide cell owns the columns past itself, so a walk that ends
    // anywhere else ends on a continuation nothing owns: the left edge, which
    // `Buffer::resize` and the shifting operations both leave behind, or a
    // narrow cell that `delete_cells` shifted in beside one. Either way it
    // belongs to no cluster and stands as its own column rather than being
    // folded into one that is not there.
    //
    // A wide cell owns exactly the columns its width accounts for, so a
    // continuation lying past the pair is unowned too, and for the same
    // reason. `cluster_end` bounds its walk the same way, which is what
    // makes the two answer alike about the same column.
    if start != here && (!line[start].is_wide() || here >= start + usize::from(line[start].width()))
    {
        return here;
    }
    start
}

/// The last column of the cluster that owns `at`.
///
/// The mirror of [`cluster_start`]. A run ending on a cell whose
/// continuations lie past it would cover part of a glyph, and which end a
/// change exposes depends on the direction it grew in, so both are closed.
pub(super) fn cluster_end(line: &[Cell], at: usize) -> usize {
    // The mirror of the ownership rule above. A cell that is not wide draws
    // one column and owns nothing past itself, so its cluster ends on it even
    // when a continuation nothing owns follows. A wide cell owns exactly the
    // columns its width accounts for and no more, so the walk stops there
    // rather than swallowing a continuation past the pair.
    let Some(cell) = line.get(at) else {
        return at;
    };
    if !cell.is_wide() {
        return at;
    }
    let owned = at + usize::from(cell.width()) - 1;
    let mut end = at;
    while end < owned && end + 1 < line.len() && line[end + 1].is_continuation() {
        end += 1;
    }
    end
}

impl Renderer {
    /// How many cells starting at `x` can be written as one batch.
    ///
    /// A batch needs every cell to be a printable one-column cell in the
    /// same style, because the pen is set once and the cursor advances
    /// once. Anything that makes a single write special is excluded: wide
    /// cells own a continuation, the last column parks the cursor in the
    /// right-margin phantom, and the bottom-right corner has its own
    /// auto-wrap dance.
    fn styled_run_len(
        &self,
        line: &[Cell],
        x: usize,
        last: usize,
        surface_width: u16,
        surface_height: u16,
    ) -> usize {
        // Bail before doing any setup when there is no run to find. Text
        // where every cell carries a different style never batches, and the
        // scan would otherwise be pure overhead on every cell.
        if x >= last || line[x].style != line[x + 1].style {
            return 1;
        }

        // A run that would reach the right margin has to go through the
        // per-cell path so the phantom-column bookkeeping still happens.
        let room = (surface_width as usize).saturating_sub(self.cur.pos().x as usize + 1);
        let limit = last.min(x + room.saturating_sub(1));
        if self.fullscreen && self.cur.pos().y + 1 == surface_height {
            // The bottom row ends in the corner case; leave it alone.
            return 1;
        }

        let style = &line[x].style;
        let mut n = 0;
        while x + n <= limit {
            let cell = &line[x + n];
            if cell.width() != 1 || cell.is_continuation() || cell.content().is_empty() {
                break;
            }
            if &cell.style != style {
                break;
            }
            // Stop before an equal-run: ECH and REP encode those far more
            // cheaply than literal bytes, and batching would swallow them.
            // The run must end *before* this cell, not after it, or the
            // repeat starts one cell late and costs an extra literal byte.
            // `emit_range` only calls this when `line[x] != line[x + 1]`,
            // so this cannot fire at n == 0.
            if x + n < limit && line[x + n] == line[x + n + 1] {
                break;
            }
            n += 1;
        }
        n
    }

    /// Write a run of same-styled one-column cells in one go.
    fn put_styled_run(
        &mut self,
        out: &mut Vec<u8>,
        run: &[Cell],
        surface_width: u16,
        surface_height: u16,
    ) -> io::Result<()> {
        self.update_pen(out, Some(&run[0]))?;
        // Resolve the phantom column once for the whole run rather than
        // once per cell.
        if self.cur.at_phantom {
            let next_y = self.cur.pos().y.saturating_add(1);
            if next_y < surface_height {
                let target = crate::layout::Position { y: next_y, x: 0 };
                self.write_optimal_move(out, self.cur.pos(), target, None)?;
                self.cur.set_pos(target);
            }
            self.cur.at_phantom = false;
        }
        for cell in run {
            out.extend_from_slice(cell.content().as_bytes());
        }
        let advanced = self.cur.pos().x.saturating_add(run.len() as u16);
        self.cur.x = Some(advanced.min(surface_width));
        Ok(())
    }

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
        // The cursor is already parked at `first`, and a continuation emits
        // nothing and moves nothing, so a run starting on one would put the
        // next glyph a column to the left of where it belongs.
        debug_assert!(
            line.get(first).is_none_or(|c| !c.is_continuation())
                || first == 0
                || !line[first - 1].is_wide(),
            "emit_range starts on a continuation at column {first}, and a cell owns it"
        );
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

            // Singleton: the cell differs from its successor. Ordinary
            // text is almost all singletons, so before falling back to one
            // emission per cell, look for a run that merely shares a style.
            // Those can be written as one byte append and one cursor
            // advance instead of paying the per-cell bookkeeping.
            if line[x] != line[next_idx] {
                let run = self.styled_run_len(line, x, last, surface_width, surface_height);
                if run > 1 {
                    self.put_styled_run(out, &line[x..x + run], surface_width, surface_height)?;
                    x += run;
                    continue;
                }
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
            let cup_b =
                ansi::cost::cup_cost(self.cur.pos().y, self.cur.pos().x.saturating_add(count));
            let rep_b = ansi::cost::rep_cost(count);

            if has_ech
                && (count as usize) > ech_b + cup_b
                && can_clear_with(cell0, self.opts.contains(Optimizations::BCE))
            {
                self.update_pen(out, Some(cell0))?;
                ansi::screen::write_ech(out, count)?;
                if j > last {
                    return Ok(true);
                }
                self.move_to(
                    out,
                    new_buf,
                    self.cur.pos().y,
                    self.cur.pos().x.saturating_add(count),
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
                let wrap_possible = self.cur.pos().x.saturating_add(count) >= surface_width;
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
                    ansi::screen::write_rep(out, rep_count)?;
                    self.cur.x = Some(
                        self.cur
                            .pos()
                            .x
                            .saturating_add(rep_count)
                            .min(surface_width),
                    );
                    if self.cur.pos().x >= surface_width {
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
                let equal = old_cell.is_some_and(|o| o == new_cell);
                if equal {
                    same += 1;
                } else {
                    if same > inline {
                        // Run of matching cells exceeded the cost of an
                        // in-place cursor move; emit what we have, then
                        // skip past the run with a positioning jump.
                        //
                        // The jump lands where emission resumes, so it has
                        // to name a column that owns its cell. Landing on a
                        // continuation would park the cursor inside a glyph
                        // that emits nothing, and the next one would go out
                        // a column early. The segment just closed ends at
                        // the last column of its own cluster for the same
                        // reason, so neither end cuts a glyph in half.
                        let resume = cluster_start(new_line, j);
                        let prev_end = j.saturating_sub(same).saturating_sub(1);
                        if prev_end >= seg_start {
                            // `resume` is zero when every column back to
                            // the start of the row continues one cluster,
                            // and an unsaturated subtraction there wraps,
                            // which discards the clamp instead of applying
                            // it.
                            let stop =
                                cluster_end(new_line, prev_end).min(resume.saturating_sub(1));
                            self.emit_range(out, new_buf, new_line, seg_start, stop)?;
                        }
                        self.move_to(out, new_buf, y, resume as u16)?;
                        seg_start = resume;
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
        // `count` is a column count, and a cell can occupy two of them, so
        // the window can end between a cell and the column it owns. The
        // cells are written whole either way, so the room has to cover the
        // whole of the last one: under insert mode each glyph opens its own
        // room as it arrives, and a glyph wider than the room left carries
        // the tail one column further than was asked for.
        let count = match line.get(count - 1) {
            Some(_) => cluster_end(line, count - 1) + 1,
            None => count,
        };
        let use_ich = self.opts.contains(Optimizations::ICH);
        if use_ich {
            ansi::screen::write_ich(out, count as u16)?;
        } else {
            ansi::mode::write_set_mode(out, &[ansi::mode::Mode::INSERT])?;
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
            ansi::mode::write_reset_mode(out, &[ansi::mode::Mode::INSERT])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod cluster_bounds {
    use super::*;

    /// A row of two-column clusters: column `2n` owns one, `2n + 1`
    /// continues it.
    fn line() -> Vec<Cell> {
        (0..6)
            .map(|i| {
                if i % 2 == 0 {
                    Cell::wide("\u{4e16}")
                } else {
                    Cell::continuation()
                }
            })
            .collect()
    }

    /// Only a wide cell owns the column to its right. `delete_cells`
    /// shifting a row left past a wide lead leaves its continuation beside
    /// whatever narrow cell now precedes it, and neither helper may credit
    /// that narrow cell with a two-column cluster.
    #[test]
    fn a_narrow_cell_does_not_own_a_following_continuation() {
        let line = vec![Cell::narrow("a"), Cell::continuation()];
        assert_eq!(cluster_start(&line, 1), 1);
        assert_eq!(cluster_end(&line, 0), 0);
    }

    /// A wide cell owns the columns its width accounts for and no more, so a
    /// continuation past the pair belongs to no cluster and the close stops
    /// short of it. `Buffer::resize` and the row shifts both leave such a row.
    #[test]
    fn a_cluster_closes_on_the_columns_its_owner_accounts_for() {
        let chained = vec![
            Cell::wide("\u{4e16}"),
            Cell::continuation(),
            Cell::continuation(),
        ];
        assert_eq!(
            cluster_end(&chained, 0),
            1,
            "the pair, not the orphan past it"
        );
        assert_eq!(cluster_end(&chained, 1), 1, "already the last owned column");
        assert_eq!(cluster_end(&chained, 2), 2, "an orphan stands alone");

        // `cluster_start` is the mirror, so it has to answer alike about the
        // same column: an orphan past the pair opens its own cluster, and a
        // column the pair does own still folds back into it.
        assert_eq!(
            cluster_start(&chained, 2),
            2,
            "an orphan past the pair stands alone"
        );
        assert_eq!(cluster_start(&chained, 1), 0, "inside the pair");
        assert_eq!(
            cluster_start(&chained, 2),
            cluster_end(&chained, 2),
            "the mirrors must agree on a column that is its own cluster"
        );
    }

    #[test]
    fn a_column_owning_its_cell_is_already_closed() {
        let line = line();
        assert_eq!(cluster_start(&line, 2), 2);
        assert_eq!(cluster_end(&line, 2), 3);
    }

    #[test]
    fn a_continuation_closes_back_to_the_cell_that_owns_it() {
        let line = line();
        assert_eq!(cluster_start(&line, 3), 2);
        assert_eq!(cluster_start(&line, 5), 4);
    }

    #[test]
    fn closing_stays_inside_the_row() {
        let line = line();
        assert_eq!(cluster_start(&line, 0), 0);
        assert_eq!(cluster_end(&line, 5), 5);
        assert_eq!(cluster_start(&[], 3), 0);
        assert_eq!(cluster_end(&line, 99), 99);
    }

    /// A continuation that reaches the left edge without meeting a cell
    /// that owns it belongs to no cluster, so it stands as its own column.
    ///
    /// Folding it into a cluster that is not there would name a column the
    /// row does not start a glyph at, and the shifting operations and
    /// `Buffer::resize` can both leave such a row behind.
    #[test]
    fn an_unowned_continuation_stands_on_its_own() {
        let line = vec![Cell::continuation(); 3];
        assert_eq!(cluster_start(&line, 2), 2);
        assert_eq!(cluster_start(&line, 0), 0);

        // One with an owner still closes back to it.
        let owned = vec![Cell::wide("\u{4e16}"), Cell::continuation()];
        assert_eq!(cluster_start(&owned, 1), 0);
    }
}

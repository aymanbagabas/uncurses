//! Per-line transform algorithm — the diff loop and its core helpers
//! (`transform_line`, `transform_line_taken`, `transform_line_inner`).

use std::io::{self, Write};

use super::predicates::{can_clear_with, cells_eq_diff};
use crate::ansi;
use crate::cell::Cell;
use crate::renderer::caps::Optimizations;
use crate::renderer::{RenderBuffer, Renderer};

impl Renderer {
    /// Transform a single line — the core diff algorithm.
    ///
    /// Compares old vs new cells and writes the minimal escape sequence to update.
    /// `y` is the 0-based row; `first_x`/`last_x` are 0-based column bounds (inclusive).
    pub(crate) fn transform_line(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &RenderBuffer,
        y: u16,
        first_x: u16,
        last_x: u16,
    ) -> io::Result<()> {
        // Pull cur_buf out of self so the inner pass can borrow a row
        // mutably while still calling &mut self methods on the renderer.
        // None of the methods reached from transform_line_inner touch
        // self.cur_buf, so this is safe.
        let mut cur_buf = self.cur_buf.take();
        let result = self.transform_line_taken(out, new_buf, &mut cur_buf, y, first_x, last_x);
        self.cur_buf = cur_buf;
        result
    }

    fn transform_line_taken(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &RenderBuffer,
        cur_buf: &mut Option<RenderBuffer>,
        y: u16,
        first_x: u16,
        last_x: u16,
    ) -> io::Result<()> {
        // Pre-compute the row's rightmost externally-painted
        // placeholder column (across both buffers). Cell-shifting
        // optimizations (ICH/DCH) acting on a column at or before
        // it would slide a placeholder off its anchor, so the inner
        // pass refuses them; operations strictly to the right stay
        // eligible.
        let last_skip = match (
            new_buf.last_skip_col(y),
            cur_buf.as_ref().and_then(|b| b.last_skip_col(y)),
        ) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        let cur_line = cur_buf.as_mut().and_then(|cb| cb.line_mut(y));
        let first_diff =
            self.transform_line_inner(out, new_buf, cur_line, y, first_x, last_x, last_skip)?;
        if let Some(first_diff) = first_diff
            && let Some(cur) = cur_buf.as_mut()
            && let (Some(new_line), Some(cur_line)) = (new_buf.line(y), cur.line_mut(y))
        {
            let len = cur_line.len().min(new_line.len());
            if first_diff < cur_line.len() && first_diff < new_line.len() {
                cur_line[first_diff..len].clone_from_slice(&new_line[first_diff..len]);
            } else if len > 0 {
                // Reference falls back to refreshing the entire short
                // line from index 0 when first_diff is past one of the
                // lines, so cur_buf stays consistent with the screen
                // when a resize leaves cur_line shorter than the new
                // line and the emission already covered the leading
                // columns.
                cur_line[..len].clone_from_slice(&new_line[..len]);
            }
        }
        Ok(())
    }

    /// Transform a single row, mirroring the canonical algorithm's
    /// structure: anchor on first/last non-blank columns of old and new
    /// lines, then dispatch to one of five branches based on what kind
    /// of change pattern the row exhibits.
    ///
    /// Returns `Some(first_cell)` when output was emitted, where
    /// `first_cell` is the leftmost column the screen was updated from
    /// — the wrapper uses this to slice-copy `new_line[first_cell..]`
    /// into `cur_buf` so it tracks what is now on screen.
    #[allow(clippy::too_many_arguments)]
    fn transform_line_inner(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &RenderBuffer,
        mut cur_line: Option<&mut [Cell]>,
        y: u16,
        _first_x: u16,
        _last_x: u16,
        last_skip: Option<u16>,
    ) -> io::Result<Option<usize>> {
        let width = new_buf.width() as usize;
        let new_line = match new_buf.line(y) {
            Some(l) => l,
            None => return Ok(None),
        };
        if width == 0 || new_line.is_empty() {
            return Ok(None);
        }

        // === Step 1: find firstCell ===
        //
        // When the new row begins with cells that the terminal can
        // reproduce by erasing (default-style blanks), we may be able
        // to use EL-1 to wipe a leading run. Otherwise just scan
        // linearly for the first differing cell.
        let leading_blank: &Cell = &new_line[0];
        let mut first_cell;
        // `copy_from` is the leftmost column where the post-emission
        // screen matches `new_line`. The wrapper uses it to copy
        // new_line[copy_from..] into cur_buf. For most paths it equals
        // first_cell, but the EL-1 emission also clears columns to the
        // left of first_cell, so copy_from stays at the pre-erase
        // first_cell in that case.
        let copy_from;
        if can_clear_with(leading_blank, self.opts.contains(Optimizations::BCE)) {
            let mut o_first = 0usize;
            if let Some(cur) = cur_line.as_deref() {
                while o_first < cur.len() && &cur[o_first] == leading_blank {
                    o_first += 1;
                }
            }
            let mut n_first = 0usize;
            while n_first < new_line.len() && &new_line[n_first] == leading_blank {
                n_first += 1;
            }

            if n_first == o_first {
                // Same number of leading blanks on each side: scan
                // forward from there for the first real diff.
                first_cell = n_first;
                while first_cell < width {
                    let new_c = &new_line[first_cell];
                    let old_c = cur_line.as_deref().and_then(|c| c.get(first_cell));
                    if old_c.is_none_or(|o| !cells_eq_diff(o, new_c)) {
                        break;
                    }
                    first_cell += 1;
                }
                copy_from = first_cell;
            } else if o_first > n_first {
                // Old had more leading blanks; nothing to clear.
                first_cell = n_first;
                copy_from = first_cell;
            } else {
                // o_first < n_first: new row has a wider leading-blank
                // run than old. Consider EL-1 (or EL-0 if it covers
                // the whole row); fall through to the trailing logic
                // either way.
                first_cell = o_first;
                copy_from = first_cell;
                let leading = n_first - o_first;
                const EL1_COST: usize = 4; // ESC [ 1 K
                if EL1_COST < leading {
                    if n_first >= width {
                        self.move_to(out, new_buf, y, 0)?;
                        self.update_pen(out, Some(leading_blank))?;
                        ansi::write_erase_to_eol(out)?;
                    } else {
                        self.move_to(out, new_buf, y, (n_first - 1) as u16)?;
                        self.update_pen(out, Some(leading_blank))?;
                        out.write_all(ansi::screen::ERASE_LINE_LEFT)?;
                    }
                    // Mirror the cleared region into the borrowed old
                    // line so subsequent comparisons (notably the
                    // walk-back in step 4c) see the same screen state
                    // the terminal has after the erase. BCE paints
                    // bg-only into the cleared cells, so model the
                    // fill that way rather than copying leading_blank
                    // verbatim (which would carry fg / attrs the wire
                    // never received). Split-borrow on `self.cur` is
                    // disjoint from `cur_line` (already split off
                    // above), so the bce_blank ref lives directly.
                    let bce = self.opts.contains(Optimizations::BCE);
                    let bce_fill: &Cell = self.cur.bce_blank(bce);
                    if let Some(cur) = cur_line.as_deref_mut() {
                        let end = n_first.min(cur.len());
                        if first_cell < end {
                            crate::buffer::fill_line_into(&mut cur[first_cell..end], bce_fill);
                        }
                    }
                    // After the erase the cleared columns match `new`;
                    // advance firstCell past them so the trailing logic
                    // can focus on the rest of the row. copy_from stays
                    // at the pre-erase column so the wrapper also
                    // refreshes cur_buf for the cleared region.
                    first_cell = n_first;
                }
            }
        } else {
            first_cell = 0;
            while first_cell < width {
                let new_c = &new_line[first_cell];
                let old_c = cur_line.as_deref().and_then(|c| c.get(first_cell));
                if old_c.is_none_or(|o| !cells_eq_diff(o, new_c)) {
                    break;
                }
                first_cell += 1;
            }
            copy_from = first_cell;
        }

        if first_cell >= width {
            return Ok(None);
        }

        let cur_slice = cur_line.as_deref();

        // === Step 2: trailing-uncolorable fast path ===
        //
        // When the rightmost cell of the new row is non-default
        // (colored / styled / non-space), the terminal can't safely
        // wipe a trailing region with EL or DCH — those fill with the
        // current pen, which would visibly alter the right margin. In
        // that case just walk backward to find the last differing
        // column and overwrite the changed range.
        let trailing = &new_line[width - 1];
        if !can_clear_with(trailing, self.opts.contains(Optimizations::BCE)) {
            let mut n_last = width - 1;
            while n_last > first_cell {
                let new_c = &new_line[n_last];
                let old_c = cur_slice.and_then(|c| c.get(n_last));
                if old_c.is_some_and(|o| cells_eq_diff(o, new_c)) {
                    n_last -= 1;
                } else {
                    break;
                }
            }
            if n_last >= first_cell {
                self.move_to(out, new_buf, y, first_cell as u16)?;
                self.put_range(out, new_buf, cur_slice, new_line, y, first_cell, n_last)?;
            }
            return Ok(Some(copy_from));
        }

        // === Step 3: locate last non-blank cells on each side ===
        let blank: &Cell = trailing;
        let mut o_last = width.saturating_sub(1);
        while o_last > first_cell
            && cur_slice
                .and_then(|c| c.get(o_last))
                .is_some_and(|c| c == blank)
        {
            o_last -= 1;
        }
        let mut n_last = width - 1;
        while n_last > first_cell && &new_line[n_last] == blank {
            n_last -= 1;
        }

        let el0 = self.el0_cost();

        if n_last == first_cell && el0 < o_last.saturating_sub(n_last) {
            // === Step 4a: single non-blank cell + erase the rest ===
            self.move_to(out, new_buf, y, first_cell as u16)?;
            if &new_line[first_cell] != blank {
                self.emit_range(out, new_buf, new_line, first_cell, first_cell)?;
            }
            self.clear_to_end(out, cur_slice, blank, width, false)?;
        } else if n_last != o_last
            && !cur_slice
                .and_then(|c| c.get(o_last))
                .is_some_and(|c| new_line.get(n_last).is_some_and(|n| cells_eq_diff(c, n)))
        {
            // === Step 4b: last non-blank cells differ in both
            // position and value — overwrite a range, optionally
            // followed by EL when the old row is meaningfully longer.
            self.move_to(out, new_buf, y, first_cell as u16)?;
            if o_last > n_last && o_last - n_last > el0 {
                let eoi =
                    self.put_range(out, new_buf, cur_slice, new_line, y, first_cell, n_last)?;
                if eoi {
                    self.move_to(out, new_buf, y, (n_last + 1) as u16)?;
                }
                self.clear_to_end(out, cur_slice, blank, width, false)?;
            } else {
                let n = n_last.max(o_last);
                self.put_range(out, new_buf, cur_slice, new_line, y, first_cell, n)?;
            }
        } else {
            // === Step 4c: tail walk-back + ICH/DCH ===
            //
            // The last non-blank cells match either in position or in
            // value. Walk back over matching cells (in lockstep) to
            // find where the lines really diverge, then either insert
            // or delete the difference in line length with ICH/DCH.
            let n_last_nonblank = n_last;
            let o_last_nonblank = o_last;
            let mut n_lc = n_last as isize;
            let mut o_lc = o_last as isize;
            fn cell_at_isize(line: &[Cell], i: isize) -> Option<&Cell> {
                if i < 0 { None } else { line.get(i as usize) }
            }
            let cur_at_isize =
                |i: isize| -> Option<&Cell> { cur_slice.and_then(|c| cell_at_isize(c, i)) };
            // Treat out-of-bounds slots as unequal so the walk-back
            // stops at the line boundary rather than stepping past it.
            // Returning false when either side is absent makes the loop
            // break at At(-1) instead of decrementing both indices to
            // -1.
            fn cells_eq(a: Option<&Cell>, b: Option<&Cell>) -> bool {
                matches!((a, b), (Some(x), Some(y)) if cells_eq_diff(x, y))
            }
            while cells_eq(cell_at_isize(new_line, n_lc), cur_at_isize(o_lc)) {
                if !cells_eq(cell_at_isize(new_line, n_lc - 1), cur_at_isize(o_lc - 1)) {
                    break;
                }
                n_lc -= 1;
                o_lc -= 1;
                if n_lc == -1 || o_lc == -1 {
                    break;
                }
            }

            let n = o_lc.min(n_lc);
            if n >= first_cell as isize {
                self.move_to(out, new_buf, y, first_cell as u16)?;
                self.put_range(out, new_buf, cur_slice, new_line, y, first_cell, n as usize)?;
            }

            if o_lc < n_lc {
                // Insertion: new row has more non-blank cells than old.
                let m = n_last_nonblank.max(o_last_nonblank);
                // Wide-cell adjustment: ICH must land on a cell
                // boundary, never in the middle of a wide character.
                // If `n+1` would point at a continuation cell, shift
                // `n` so the cursor lands past the wide cell (advance)
                // when we're already at the leftmost column, or back
                // up before it otherwise.
                let mut n = n;
                let mut o_lc = o_lc;
                if n != 0 {
                    while n > 0 {
                        match cell_at_isize(new_line, n + 1) {
                            Some(w) if w.is_continuation() => {
                                n -= 1;
                                o_lc -= 1;
                            }
                            _ => break,
                        }
                    }
                } else if n >= first_cell as isize
                    && cell_at_isize(new_line, n).is_some_and(|c| c.is_wide())
                {
                    while cell_at_isize(new_line, n + 1).is_some_and(|c| c.is_continuation()) {
                        n += 1;
                        o_lc += 1;
                    }
                }
                self.move_to(out, new_buf, y, (n + 1) as u16)?;
                let ich_count = (n_lc - o_lc) as usize;
                let ich_cost = ansi::cost::ich_cost(ich_count as u16);
                let span = (m as isize - n) as usize;
                // Prefer plain overwrite when ICH is available but the
                // walk-back ended before the last non-blank cell — there
                // is content past n_lc that ICH wouldn't cover and we'd
                // have to write anyway — or the ICH sequence costs more
                // than just emitting the m-n cells. Otherwise insert via
                // ICH or IRM depending on terminal support.
                // Shift starts at column `n + 1`. Refuse the
                // optimization when any externally-painted
                // placeholder sits at or past that column — the
                // shift would slide it off its anchor.
                let shift_col = (n + 1) as u16;
                let skip_blocks = last_skip.is_some_and(|c| c >= shift_col);
                if skip_blocks
                    || (self.opts.contains(Optimizations::ICH)
                        && ((n_lc as usize) < n_last_nonblank || ich_cost > span))
                {
                    self.put_range(out, new_buf, cur_slice, new_line, y, (n + 1) as usize, m)?;
                } else {
                    self.insert_cells_op(out, &new_line[(n + 1) as usize..], ich_count)?;
                }
            } else if o_lc > n_lc {
                // Deletion: new row has fewer non-blank cells than old.
                self.move_to(out, new_buf, y, (n + 1) as u16)?;
                let dch_count = (o_lc - n_lc) as usize;
                let dch_cost = ansi::cost::dch_cost(dch_count as u16) as isize;
                let el_cost = ansi::cost::el_cost(0) as isize;
                let tail = n_last_nonblank as isize - (n + 1);
                let shift_col = (n + 1) as u16;
                let skip_blocks = last_skip.is_some_and(|c| c >= shift_col);
                if !self.opts.contains(Optimizations::DCH)
                    || dch_cost > el_cost + tail
                    || skip_blocks
                {
                    // (n+1) may exceed n_last_nonblank when the
                    // walk-back left n_lc at n_last_nonblank; the
                    // reference relies on Go's signed arithmetic so
                    // putRange becomes a no-op for the inverted span.
                    if (n + 1) <= n_last_nonblank as isize {
                        let eoi = self.put_range(
                            out,
                            new_buf,
                            cur_slice,
                            new_line,
                            y,
                            (n + 1) as usize,
                            n_last_nonblank,
                        )?;
                        if eoi {
                            self.move_to(out, new_buf, y, (n_last_nonblank + 1) as u16)?;
                        }
                    }
                    self.clear_to_end(out, cur_slice, blank, width, false)?;
                } else {
                    self.update_pen(out, Some(blank))?;
                    ansi::write_dch(out, dch_count as u16)?;
                }
            }
        }

        Ok(Some(copy_from))
    }
}

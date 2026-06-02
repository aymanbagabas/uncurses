//! Whole-region clears (`clear_bottom`, `clear_to_end`) and the
//! `el0_cost` heuristic that drives them.

use std::io;

use super::predicates::{can_clear_with, cells_equal_blank};
use crate::Position;
use crate::ansi::{self, cursor as ansi_cursor};
use crate::buffer::{SurfaceMut, fill_line_into};
use crate::cell::Cell;
use crate::renderer::caps::Optimizations;
use crate::renderer::{RenderBuffer, Renderer};

impl Renderer {
    /// Clear trailing blank lines from the bottom using ED (erase display).
    ///
    /// Returns the "top" row of the trimmed region — the number of rows
    /// the caller still needs to walk through `transform_line`. When no
    /// trimming happens, this is `new_buf.height()`.
    ///
    /// Only fires when EVERY cell across the trailing rows in both
    /// `new_buf` AND `cur_buf` matches the current pen's blank cell,
    /// AND the blank is reproducible by erase: with BCE the active
    /// background is preserved, without BCE the blank must have a
    /// default background (otherwise the terminal would paint the
    /// cleared region with the wrong color).
    pub(crate) fn clear_bottom(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &RenderBuffer,
    ) -> io::Result<usize> {
        let height = new_buf.height();
        if !self.cur.style().is_link_empty() {
            return Ok(height as usize);
        }
        let bce = self.opts.contains(Optimizations::BCE);
        // Only proceed if the current pen is reproducible via ED at all.
        if !can_clear_with(self.cur.current_blank(), bce) {
            return Ok(height as usize);
        }

        // Walk rows from the bottom up. While both buffers' trailing
        // rows are blank, track the bottom-most row in new_buf's
        // trailing-blank zone where cur_buf was actually non-blank —
        // that's the only case an ED has anything to wipe. Once
        // new_buf goes non-blank, rows above can't contribute to ED.
        // If no such row is found (cur is already blank wherever new
        // is blank, e.g. after a force-clear or on the first frame),
        // the ED would be redundant and we skip it.
        let mut last_nonblank = height;
        let mut cur_clear_after: Option<u16> = None;
        let has_cur = self.cur_buf.is_some();
        // Split-borrow: borrowing `self.cur` for the blank template is
        // disjoint from `self.cur_buf` reads below, so the loop runs
        // without cloning the cell.
        {
            let blank: &Cell = self.cur.current_blank();
            for y in (0..height).rev() {
                let new_all_blank = match new_buf.line(y) {
                    Some(l) => l.iter().all(|c| cells_equal_blank(c, blank)),
                    None => true,
                };
                if !new_all_blank {
                    last_nonblank = y + 1;
                    break;
                }
                if has_cur && cur_clear_after.is_none() {
                    let cb = self.cur_buf.as_ref().unwrap();
                    let cur_all_blank = match cb.line(y) {
                        Some(l) => l.iter().all(|c| cells_equal_blank(c, blank)),
                        None => true,
                    };
                    if !cur_all_blank {
                        cur_clear_after = Some(y + 1);
                    }
                }
            }
        }

        if last_nonblank == height {
            return Ok(height as usize);
        }

        if cur_clear_after.is_some() {
            self.clear_below(out, new_buf, last_nonblank)?;
            // Sync hashes for the now-blank rows: scroll detection on
            // the next frame must see these rows as already-blank in
            // the old hashmap, otherwise it can spuriously decide
            // there's stale content to scroll away.
            if !self.old_hashes.is_empty() && self.old_hashes.len() == self.new_hashes.len() {
                let h = self.old_hashes.len();
                for y in (last_nonblank as usize)..h {
                    self.old_hashes[y] = self.new_hashes[y];
                }
            }
        }

        Ok(last_nonblank as usize)
    }

    /// Emit `\e[H\e[2J` (cursor home + erase entire screen) and sync
    /// `cur_buf` to the resulting blank state. The active pen's bg is
    /// what BCE leaves on every cell.
    pub(crate) fn clear_screen(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        ansi_cursor::write_cup(out, 0, 0)?;
        ansi::write_erase_screen(out)?;
        self.cur.pos = Position { y: 0, x: 0 };
        self.cur.at_phantom = false;
        // The CUP just emitted authoritatively places the tracked
        // cursor at (0,0); mark both axes known so the next move_to
        // can hit the same-position early-return instead of forcing
        // a redundant absolute CUP.
        self.cur.x_unknown = false;
        self.cur.y_unknown = false;

        let bce = self.opts.contains(Optimizations::BCE);
        let blank: Cell = self.cur.bce_blank(bce).clone();
        if let Some(cb) = self.cur_buf.as_mut() {
            cb.fill(&blank);
        }
        Ok(())
    }

    /// Move to `(row, 0)` and clear from there to the bottom of the
    /// surface. Convenience wrapper around [`Self::clear_to_bottom`]
    /// for the common "wipe rows from `row` down" case.
    pub(crate) fn clear_below(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &RenderBuffer,
        row: u16,
    ) -> io::Result<()> {
        self.move_to(out, new_buf, row, 0)?;
        self.clear_to_bottom(out)
    }

    /// Emit ED-below from the current cursor position and sync `cur_buf`
    /// to match.
    ///
    /// Mirrors the wire effect of ED 0: the terminal blanks `(col..width)`
    /// of the cursor row, then every row below, using the current pen
    /// (BCE-aware). `cur_buf` is updated identically so the next frame's
    /// diff sees the post-erase state. The caller is responsible for
    /// moving the cursor first and for any hash bookkeeping.
    pub(crate) fn clear_to_bottom(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        ansi::write_erase_below(out)?;

        let row = self.cur.pos.y;
        let col = self.cur.pos.x;
        let height = match self.cur_buf.as_ref() {
            Some(cb) => cb.height(),
            None => 0,
        };
        // BCE leaves the active pen's bg on every cleared cell;
        // without BCE the cleared cells revert to default. Mirror that
        // exactly in cur_buf so the next frame's diff is accurate AND
        // skips re-emit when the same blank persists.
        let bce = self.opts.contains(Optimizations::BCE);
        let blank: Cell = self.cur.bce_blank(bce).clone();
        if let Some(cb) = self.cur_buf.as_mut() {
            // Cursor row: blank from col to end of line.
            if let Some(line) = cb.line_mut(row)
                && (col as usize) < line.len()
            {
                fill_line_into(&mut line[col as usize..], &blank);
            }
            // Every row below: blank entirely.
            for y in row.saturating_add(1)..height {
                if let Some(line) = cb.line_mut(y) {
                    fill_line_into(line, &blank);
                }
            }
        }
        Ok(())
    }

    /// Byte cost of an EL-0 (erase to EOL) sequence relative to writing
    /// space cells. When the terminal supports background-color erase,
    /// EL paints the cleared region with the current pen and we treat
    /// it as free so cost comparisons fall through to "prefer erase"
    /// for any non-empty trailing region. Without BCE we fall back to
    /// the literal sequence length so small trailing runs are emitted
    /// as explicit blanks instead.
    pub(super) fn el0_cost(&self) -> usize {
        if self.opts.contains(Optimizations::BCE) {
            0
        } else {
            ansi::cost::el_cost(0)
        }
    }

    /// Clear from the current cursor column to the right margin of the
    /// current row. When `force` is false, this is a no-op if the old
    /// buffer already has the trailing region matching `blank` at every
    /// position. Picks EL-0 over a run of explicit blanks when the EL
    /// byte cost is no larger than the fill cost.
    pub(super) fn clear_to_end(
        &mut self,
        out: &mut Vec<u8>,
        old_line: Option<&[Cell]>,
        blank: &Cell,
        width: usize,
        force: bool,
    ) -> io::Result<()> {
        let cur_x = self.cur.pos.x as usize;
        let need = force
            || match old_line {
                Some(cur) => (cur_x..width).any(|j| j >= cur.len() || cur[j] != *blank),
                None => true,
            };
        if !need {
            return Ok(());
        }
        self.update_pen(out, Some(blank))?;
        let count = width.saturating_sub(cur_x);
        if self.el0_cost() <= count {
            ansi::write_erase_to_eol(out)?;
        } else if count > 0 {
            // Bulk-emit the ASCII space fill. Going through
            // put_glyph_bytes per byte paid the phantom check + the
            // bottom-right-corner check `count` times even though
            // only the final byte can land on the corner. Emit the
            // leading run in one resize() (memset), then route the
            // last space through put_glyph_bytes so the corner
            // auto-wrap dance / phantom bookkeeping still runs.
            let surface_width = self.last_width;
            let surface_height = self.last_height;
            if count > 1 {
                let leading = count - 1;
                out.resize(out.len() + leading, b' ');
                self.cur.pos.x = self.cur.pos.x.saturating_add(leading as u16);
            }
            self.put_glyph_bytes(out, b" ", 1, surface_width, surface_height)?;
        }
        Ok(())
    }
}

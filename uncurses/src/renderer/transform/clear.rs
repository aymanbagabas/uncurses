//! Whole-region clears (`clear_bottom`, `clear_to_end`) and the
//! `el0_cost` heuristic that drives them.

use std::io;

use super::predicates::{can_clear_with, cells_equal_blank};
use crate::ansi;
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
        if self.cur.has_link() {
            return Ok(height as usize);
        }
        let bce = self.opts.contains(Optimizations::BCE);
        // Only proceed if the current pen is reproducible via ED at all.
        if !can_clear_with(self.cur.current_blank(), bce) {
            return Ok(height as usize);
        }

        // Walk rows from the bottom up. While both buffers' trailing
        // rows are blank, track the first non-blank row each side has
        // (going up). Once new_buf goes non-blank, rows above can't
        // contribute to ED — stop. cur_buf is only checked on rows
        // new_buf has already left blank, and we keep the bottom-most
        // hit, matching the original two-pass behavior.
        let mut last_nonblank = height;
        let mut old_last_nonblank = height;
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
                if has_cur && old_last_nonblank == height {
                    let cb = self.cur_buf.as_ref().unwrap();
                    let cur_all_blank = match cb.line(y) {
                        Some(l) => l.iter().all(|c| cells_equal_blank(c, blank)),
                        None => true,
                    };
                    if !cur_all_blank {
                        old_last_nonblank = y + 1;
                    }
                }
            }
        }

        if last_nonblank == height {
            return Ok(height as usize);
        }

        if last_nonblank < old_last_nonblank {
            self.move_to(out, new_buf, last_nonblank, 0)?;
            // The active pen IS what `current_blank` represents (it's
            // rebuilt from cur.style/link whenever the pen changes), so BCE
            // already paints the cleared region in the recorded style;
            // no explicit pen sync needed here.
            ansi::write_erase_below(out)?;
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
        self.update_pen(out, Some(&blank))?;
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

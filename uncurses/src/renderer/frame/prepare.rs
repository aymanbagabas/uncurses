//! Pre-diff setup phase.
//!
//! Initialises the current buffer, reacts to surface resizes (including
//! the inline-shrink partial clear), and dispatches the force-clear /
//! scroll-detection branch before the per-row diff runs.

use std::hash::{Hash, Hasher};
use std::io;

use rustc_hash::FxHasher;

const SCROLL_OPTIMIZE_MIN_TOUCHED_LINES: usize = 2;
const SCROLL_OPTIMIZE_TOUCHED_DIVISOR: usize = 8;

use crate::Position;
use crate::ansi::{self, cursor as ansi_cursor};
use crate::cell::Cell;
use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;
use crate::renderer::scroll;

/// Hash a line's content (ignoring style) for scroll detection.
///
/// Uses a non-cryptographic hasher because the only consequence of an
/// unlucky collision is a missed scroll opportunity — the affected
/// rows fall through to direct redraw, never visual corruption.
pub(crate) fn hash_line(line: &[Cell]) -> u64 {
    let mut hasher = FxHasher::default();
    for cell in line {
        cell.content().hash(&mut hasher);
    }
    hasher.finish()
}

impl Renderer {
    /// Run the pre-diff phase. Returns whether the surface size
    /// changed this frame so [`Renderer::finalize_frame`] knows
    /// whether to snap the cursor back to the inline bottom-left.
    pub(super) fn prepare_frame(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &mut RenderBuffer,
        width: u16,
        height: u16,
    ) -> io::Result<bool> {
        // Initialize current buffer if needed. Diff against the empty
        // buffer naturally — no need to force a full clear; the
        // transform pass will emit each non-blank cell.
        if self.cur_buf.is_none() {
            self.cur_buf = Some(RenderBuffer::new(width, height));
        }

        // Handle resize. We don't force a full clear here — instead we
        // invalidate the line hashes (so scroll detection starts fresh)
        // and, when shrinking inline at the same width, emit a partial
        // clear at the new bottom row to wipe any orphan rows the
        // surface used to occupy. After the normal transform pass an
        // inline resize also snaps the cursor back to the bottom-left
        // of the new surface so subsequent inline output starts in the
        // right place.
        let prev_width = self.last_width;
        let prev_height = self.last_height;
        let resized = width != prev_width || height != prev_height;
        let inline_partial_clear = resized
            && !self.fullscreen
            && self.cur_buf.is_some()
            && width == prev_width
            && height < prev_height;
        if resized {
            if let Some(ref mut cb) = self.cur_buf {
                cb.resize(width, height);
            }
            self.old_hashes.clear();
            self.new_hashes.clear();
        }
        if width != self.tabs.width() {
            self.tabs.resize(width);
        }
        self.last_width = width;
        self.last_height = height;

        if inline_partial_clear && height > 0 && !self.force_clear {
            let last_row = height - 1;
            self.move_to(out, new_buf, last_row, 0)?;
            self.reset_pen(out)?;
            ansi::write_erase_below(out)?;
            // The erase wipes the row at last_row too; blank cur_buf
            // there so the upcoming transform pass repaints it from
            // new_buf rather than skipping on a stale equality.
            if let Some(ref mut cb) = self.cur_buf
                && let Some(line) = cb.line_mut(last_row)
            {
                for cell in line.iter_mut() {
                    *cell = Cell::BLANK;
                }
            }
        }

        if self.force_clear {
            self.clear_update(out, new_buf)?;
            self.force_clear = false;
        } else if self.scroll_optimize && self.fullscreen {
            let touched_lines = new_buf.touched_lines().count();
            let sparse_threshold = SCROLL_OPTIMIZE_MIN_TOUCHED_LINES
                .max(height as usize / SCROLL_OPTIMIZE_TOUCHED_DIVISOR);
            // Sparse updates are cheaper to redraw directly; scroll detection
            // needs row hashing and matching work that only pays off once enough
            // rows changed to make moving regions competitive.
            if touched_lines > sparse_threshold {
                self.compute_hashes(new_buf);
                self.update_hashmap(new_buf, height as usize);
                self.scroll_optimize(out, new_buf)?;
            }
        }

        Ok(resized)
    }

    /// Force-clear preface for the post-clear pipeline.
    ///
    /// Emits the screen-erase, syncs cur_buf to the resulting blank
    /// state, invalidates the scroll-detection hashes, and marks
    /// every row of `new_buf` as touched so the unconditional
    /// transform pass that runs after this method repaints the
    /// whole frame.
    fn clear_update(&mut self, out: &mut Vec<u8>, new_buf: &mut RenderBuffer) -> io::Result<()> {
        if !self.fullscreen {
            // Inline: never wipe scrollback above the surface. Walk
            // relatively back to (0,0) of the surface and erase from
            // there down. The active pen IS what the recorded blank
            // represents (current_blank is built from cur.style/link), so
            // BCE paints the cleared region consistently without an
            // explicit pen sync.
            self.move_to(out, new_buf, 0, 0)?;
            ansi::write_erase_below(out)?;
        } else {
            ansi_cursor::write_cup(out, 0, 0)?;
            ansi::write_erase_screen(out)?;
            self.cur.pos = Position { y: 0, x: 0 };
            self.cur.at_phantom = false;
            // The CUP just emitted authoritatively places the tracked
            // cursor at (0,0); mark both axes known so the next
            // move_to can hit the same-position early-return instead
            // of forcing a redundant absolute CUP.
            self.cur.x_unknown = false;
            self.cur.y_unknown = false;
        }

        // Sync cur_buf to the blank state we just left on the screen
        // (so transform_line skips rows that are already blank in
        // new_buf). When the pen is fully empty the blank is exactly
        // Cell::BLANK — assigning it is a flat memcpy with no heap
        // allocations, so fast-path that case past the styled-blank
        // clone.
        let pen_empty = self.cur.style().is_empty();
        // Split-borrow: `self.cur` is disjoint from `self.cur_buf`, so
        // the blank template ref stays live while the loop mutates
        // cur_buf's lines.
        let blank: &Cell = self.cur.current_blank();
        if let Some(cb) = self.cur_buf.as_mut() {
            for y in 0..cb.height() {
                if let Some(line) = cb.line_mut(y) {
                    if pen_empty {
                        for cell in line.iter_mut() {
                            *cell = Cell::BLANK;
                        }
                    } else {
                        for cell in line.iter_mut() {
                            *cell = blank.clone();
                        }
                    }
                }
            }
        }

        // Force the post-clear transform pass to repaint every row.
        // Without this the touched gate would skip rows the caller
        // hadn't explicitly written this frame, leaving them blank
        // on screen after the ED even when new_buf carries content
        // for them.
        new_buf.touch_all();

        Ok(())
    }

    /// Compute content hashes for each line. When both hash arrays
    /// already span `height`, only rehash rows the buffer marked as
    /// touched (rehashing both old and new from their respective
    /// buffers). Otherwise reallocate and rehash everything.
    fn compute_hashes(&mut self, new_buf: &RenderBuffer) {
        let height = new_buf.height() as usize;
        let Some(cur_buf) = self.cur_buf.as_ref() else {
            return;
        };

        let cached = self.old_hashes.len() == height && self.new_hashes.len() == height;
        if !cached {
            self.old_hashes.resize(height, 0);
            self.new_hashes.resize(height, 0);
        }

        for y in 0..height {
            // Skip rows the new buffer reports as untouched: their
            // cached hashes from the previous compute_hashes call are
            // still good enough for scroll matching. A stale value
            // can only suppress one scroll opportunity (the row would
            // be redrawn directly) — never corrupts output. First-time
            // sizing falls through to rehash every row.
            if cached && new_buf.touched(y as u16).is_none() {
                continue;
            }
            if let Some(line) = cur_buf.line(y as u16) {
                self.old_hashes[y] = hash_line(line);
            }
            if let Some(line) = new_buf.line(y as u16) {
                self.new_hashes[y] = hash_line(line);
            }
        }
    }

    /// Scroll optimization pass — delegates to the scroll-apply
    /// module once the per-row mapping has enough entries to cover
    /// the surface.
    fn scroll_optimize(&mut self, out: &mut Vec<u8>, new_buf: &mut RenderBuffer) -> io::Result<()> {
        let height = self.last_height as usize;
        if self.oldnum.len() < height {
            return Ok(());
        }

        scroll::apply::apply_scrolls(out, self, new_buf)
    }
}

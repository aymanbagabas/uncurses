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

use crate::cell::Cell;
use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;
use crate::renderer::scroll;

/// Hash a line's content (ignoring style) for scroll detection.
///
/// Uses a non-cryptographic hasher because the only consequence of an
/// unlucky collision is a missed scroll opportunity — the affected
/// rows fall through to direct redraw, never visual corruption.
///
/// Rect cells additionally hash their owning rectangle's coordinates
/// so a row of opaque rect bodies never collides with a blank row or
/// with the body of a different rect at the same position.
pub(crate) fn hash_line(line: &[Cell]) -> u64 {
    let mut hasher = FxHasher::default();
    for cell in line {
        cell.content().hash(&mut hasher);
        if let Some(rect) = cell.rect() {
            // Tag with a sentinel to avoid colliding with content
            // that happens to hash like a rect's tuple.
            "rect".hash(&mut hasher);
            rect.x.hash(&mut hasher);
            rect.y.hash(&mut hasher);
            rect.width.hash(&mut hasher);
            rect.height.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Whether `line` contains any cell that belongs to a rich-content
/// rectangle. Rows that match this predicate must not participate in
/// scroll detection or in horizontal insert/delete operations — both
/// would physically displace the addon-managed payload anchored to
/// `rect.origin`.
pub(crate) fn line_contains_rect(line: &[Cell]) -> bool {
    line.iter().any(|c| c.is_rect())
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
            self.reset_pen(out)?;
            self.clear_below(out, new_buf, last_row)?;
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
            // back to (0,0) of the surface and erase from there down.
            self.clear_below(out, new_buf, 0)?;
        } else {
            self.clear_screen(out)?;
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
            let old_line = cur_buf.line(y as u16);
            let new_line = new_buf.line(y as u16);
            // Rows that contain rect cells in either buffer must
            // never be scrolled — the addon-managed payload is
            // anchored to its absolute row, and most terminals can't
            // physically move a raster image with the line. Zero
            // both hashes so the linear-probe scroll detector treats
            // the row as a sentinel "skip" slot.
            let row_has_rect = old_line.is_some_and(line_contains_rect)
                || new_line.is_some_and(line_contains_rect);
            if let Some(line) = old_line {
                self.old_hashes[y] = if row_has_rect { 0 } else { hash_line(line) };
            }
            if let Some(line) = new_line {
                self.new_hashes[y] = if row_has_rect { 0 } else { hash_line(line) };
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

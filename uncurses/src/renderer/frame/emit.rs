//! Cursor/glyph emission helpers.
//!
//! Low-level routines that the diff loop and other phases use to
//! relocate the cursor and to push glyph bytes through the right-margin
//! phantom and lower-right corner edge cases.

use std::io;

use crate::ansi;
use crate::layout::Position;
use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;

impl Renderer {
    /// Move cursor to the given position. Runs all the pre-move
    /// bookkeeping (width autowrap, pen reset, phantom-cell snap,
    /// height clamp, no-op shortcut), then defers the actual emission
    /// to [`Renderer::move_cursor`] so the destination row is available
    /// for overwrite consideration.
    ///
    /// Most callers want this. Callers that have already done the
    /// bookkeeping and need to skip it can call
    /// [`Renderer::move_cursor`] or [`Renderer::move_cursor_to`]
    /// directly.
    pub(crate) fn move_to(
        &mut self,
        out: &mut Vec<u8>,
        buf: &RenderBuffer,
        y: u16,
        x: u16,
    ) -> io::Result<()> {
        let mut y = y;
        let mut x = x;

        // A target that exceeds the surface width is treated as a
        // wrap: y += x / width, x %= width. Keeps the cursor model
        // consistent with autowrap-on terminals.
        if self.last_width > 0 && x >= self.last_width {
            y = y.saturating_add(x / self.last_width);
            x %= self.last_width;
        }

        // Right-margin phantom: the terminal cursor is in the last
        // column with the pending-wrap flag set, but our tracked
        // column is one past the last column. A bare `\r` resyncs.
        if self.cur.at_phantom {
            out.push(b'\r');
            self.cur.x = Some(0);
            self.cur.at_phantom = false;
        }

        // Clamp the target to the surface height: we may be mid-resize
        // and `last_height` could already reflect the new size. The
        // tracked source position is left untouched — when it sits
        // outside the new bounds (e.g. a finalize-snapped cursor at
        // the bottom of the previous, taller surface), the relative
        // move planner must still emit the full CUU back to the new
        // bounds. Clamping the source would silently drop those rows
        // and leave the physical cursor below where the renderer
        // believes it is, leaving stale content above the new surface
        // on inline shrinks.
        if self.last_height > 0 {
            let max_y = self.last_height - 1;
            if y > max_y {
                y = max_y;
            }
        }

        if self.cur.x == Some(x) && self.cur.y == Some(y) {
            return Ok(());
        }

        self.move_cursor(out, buf, y, x)
    }

    /// Emit a cursor move using the destination row's cells from
    /// `buf` for overwrite consideration. The bare wrapper for
    /// `move_to`: handles only the inline-mode initial `\r` snap,
    /// then defers to the optimal-move planner. Use
    /// [`Renderer::move_to`] unless you already handled the
    /// pre-move bookkeeping yourself.
    pub(crate) fn move_cursor(
        &mut self,
        out: &mut Vec<u8>,
        buf: &RenderBuffer,
        y: u16,
        x: u16,
    ) -> io::Result<()> {
        // Inline mode + cursor fully unknown on both axes: snap to
        // column 0 with a bare `\r` so the relative move below has a
        // deterministic starting column. The row stays unknown until
        // the planner emits a vertical step. Fullscreen mode handles
        // the same condition by emitting absolute CUP from the
        // planner.
        if !self.fullscreen && self.relative_cursor && self.cur.x.is_none() && self.cur.y.is_none()
        {
            out.push(b'\r');
            // Re-home the column and assume the current physical row is the
            // top of the surface, so the relative move below only ever steps
            // downward — it can never CUU above a reflowed/handed-off cursor.
            self.cur.x = Some(0);
            self.cur.y = Some(0);
        }

        let target = Position { x, y };
        let line = buf.line(y);
        self.write_optimal_move(out, self.cur.pos(), target, line)?;
        self.cur.set_pos(target);
        Ok(())
    }

    /// Mark the tracked cursor position as no longer matching the
    /// terminal on either axis. The next [`Renderer::move_to`]
    /// reasserts position.
    pub(crate) fn invalidate_cursor(&mut self) {
        self.cur.x = None;
        self.cur.y = None;
    }

    /// Write a single grapheme to the output buffer, handling the
    /// right-margin auto-wrap "phantom" state and protecting the lower
    /// right corner from triggering an unwanted scroll in fullscreen mode.
    ///
    /// Returns whether the cell was written. `width` here is the buffer
    /// width — the column count of the surface we render into.
    pub(crate) fn put_glyph_bytes(
        &mut self,
        out: &mut Vec<u8>,
        content: &[u8],
        cell_width: u16,
        surface_width: u16,
        surface_height: u16,
    ) -> io::Result<()> {
        if cell_width == 0 {
            return Ok(());
        }

        // If we are sitting in the right-margin phantom cell from a prior
        // write, advancing the cursor before writing this glyph would
        // happen via terminal auto-wrap. We don't want to depend on that
        // implicit behavior — emit an explicit move to the start of the
        // next row so the result is the same on every terminal.
        if self.cur.at_phantom {
            let next_y = self.cur.pos().y.saturating_add(1);
            if next_y < surface_height {
                let target = Position { y: next_y, x: 0 };
                self.write_optimal_move(out, self.cur.pos(), target, None)?;
                self.cur.set_pos(target);
            }
            self.cur.at_phantom = false;
        }

        // True lower-right corner: cursor sitting at (width-1, height-1)
        // with a single-column cell about to be written. Multi-column
        // cells that merely *reach* the last column don't apply — a
        // width-2 cell starting at width-2 occupies the last column but
        // the cursor itself is at width-2, not width-1, so writing it
        // doesn't trigger the bottom-right scroll quirk.
        let is_lower_right_corner = self.fullscreen
            && cell_width == 1
            && self.cur.pos().x + 1 == surface_width
            && self.cur.pos().y + 1 == surface_height;

        if is_lower_right_corner {
            // Writing the bottom-right corner in alt-screen normally pushes
            // the cursor into pending-wrap. The very next emission (or
            // sometimes even the next CSI) can then provoke an auto-wrap
            // that scrolls the alt screen up by one row. Disable auto-wrap
            // for just this glyph and restore it immediately.
            ansi::mode::Mode::AUTO_WRAP.reset(out)?;
            out.extend_from_slice(content);
            ansi::mode::Mode::AUTO_WRAP.set(out)?;
            // Cursor stays at the corner — explicitly. No phantom.
            // (Some terminals leave it at width-1, others at width; treat
            // it as width-1 since auto-wrap is now off and a subsequent
            // print would just overwrite the same cell.)
            self.cur.x = Some(surface_width.saturating_sub(1));
            self.cur.at_phantom = false;
            return Ok(());
        }

        out.extend_from_slice(content);
        let new_x = self.cur.pos().x.saturating_add(cell_width);
        if new_x >= surface_width {
            // Park at the right-margin phantom; clamp tracked column so it
            // doesn't drift past `surface_width` over successive writes.
            self.cur.x = Some(surface_width);
            self.cur.at_phantom = true;
        } else {
            self.cur.x = Some(new_x);
        }
        Ok(())
    }
}

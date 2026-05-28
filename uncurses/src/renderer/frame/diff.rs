//! Per-row diff phase.
//!
//! Trims trailing blank rows with a single ED-below where possible,
//! then walks the touched rows and emits the per-row diff via
//! [`Renderer::transform_line`].

use std::io;

use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;

impl Renderer {
    /// Run the line diff pass for the current frame.
    pub(super) fn diff_frame(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &mut RenderBuffer,
        width: u16,
        height: u16,
    ) -> io::Result<()> {
        // Clear trailing blank lines from the bottom (inline or
        // fullscreen). When new_buf has more trailing-blank rows
        // than cur_buf, emit a single ED-below at the new boundary
        // instead of repainting each row with blanks. The returned
        // `top` caps the per-row transform loop so rows below the
        // ED'd boundary aren't redundantly repainted.
        let non_empty = self.clear_bottom(out, new_buf)?;

        // Transform touched lines. We use the touched flag to skip
        // rows that have no changes on either side, but inside
        // transform_line we scan the FULL row to compare against
        // cur_buf: cur_buf can have been modified independently
        // (e.g. by scroll_optimize shifting rows) and we have to
        // pick up those differences too.
        //
        // transform_line itself keeps cur_buf in sync for the row
        // incrementally, so no post-pass clone is needed here.
        let last = width.saturating_sub(1);
        for y in 0..(non_empty as u16).min(height) {
            if new_buf.touched(y).is_some() {
                self.transform_line(out, new_buf, y, 0, last)?;
            }
        }

        Ok(())
    }
}

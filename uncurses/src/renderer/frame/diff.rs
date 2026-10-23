//! Per-row diff phase.
//!
//! Trims trailing blank rows with a single ED-below where possible,
//! then walks the touched rows and emits the per-row diff via
//! [`Renderer::transform_line`]. Placement damage requires full-row erasure.
//! After those rows are ready, emit placement bytes and their overlay cells.

use std::io;

use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;

impl Renderer {
    /// Run the line diff pass for the current frame.
    pub(in crate::renderer) fn diff_frame(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &mut RenderBuffer,
        width: u16,
        height: u16,
    ) -> io::Result<()> {
        let placements = self.placements.frame.take();
        let result = (|| {
            // Clear trailing blank lines from the bottom (inline or
            // fullscreen). When new_buf has more trailing-blank rows
            // than cur_buf, emit a single ED-below at the new boundary
            // instead of repainting each row with blanks. The returned
            // `top` caps the per-row transform loop so rows below the
            // ED'd boundary aren't redundantly repainted.
            let non_empty = if self.protected_rows.iter().any(|&row| row) {
                height as usize
            } else {
                self.clear_bottom(out, new_buf)?
            };

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
                if placements
                    .as_ref()
                    .is_some_and(|frame| frame.rows.contains_key(&y))
                {
                    let backing = new_buf.line(y).unwrap();
                    self.paint_complete_row(out, y, backing)?;
                    self.record_row(y, backing);
                    continue;
                }
                if self.row_protected(y) {
                    continue;
                }
                if self.invalid_rows.get(y as usize) == Some(&true) {
                    if let Some(line) = new_buf.line(y) {
                        self.paint_complete_row(out, y, line)?;
                        self.record_row(y, line);
                    }
                } else if new_buf.touched(y).is_some() {
                    self.transform_line(out, new_buf, y, 0, last)?;
                }
            }

            if let Some(frame) = &placements {
                // Clear all damaged rows before drawing a multi-row payload.
                // Damage spans each placement, so its first row identifies replay.
                for record in frame
                    .active
                    .iter()
                    .filter(|r| r.options.cells.is_none() && frame.rows.contains_key(&r.area.y))
                {
                    self.write_placement(
                        out,
                        crate::layout::Size::new(width, height),
                        (record.area.x, record.area.y).into(),
                        &record.options.draw,
                    )?;
                }
                for (&y, overlay) in &frame.rows {
                    if frame.coverage.contains_key(&y) {
                        self.draw_overlay(out, y, overlay, &frame.active)?;
                    }
                }
            }
            Ok(())
        })();
        self.placements.frame = placements;
        result
    }
}

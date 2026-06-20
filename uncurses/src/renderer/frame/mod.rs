//! Frame rendering pipeline.
//!
//! The top-level [`Renderer::render`] orchestrator splits frame
//! production into four phases that each live in their own submodule:
//!
//! - [`prepare`] – pre-frame setup: cur_buf initialisation, resize
//!   handling, inline-shrink partial clears, force-clear preface,
//!   and the scroll-detection pass.
//! - [`diff`] – the line-by-line diff: bottom-trim ED followed by
//!   the per-row transform loop.
//! - [`emit`] – low-level cursor and glyph emission helpers used by
//!   both the diff loop and the rest of the renderer.
//! - [`finalize`] – per-frame epilogue: inline-resize cursor snap and
//!   pen/link reset.

pub(super) mod diff;
pub(super) mod emit;
pub(super) mod finalize;
pub(super) mod prepare;

use std::io;

use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;

impl Renderer {
    /// Force a full clear/redraw on the next render.
    ///
    /// # Behavior
    ///
    /// Sets a pending flag consumed by [`Renderer::render`]. The next
    /// render emits the layout-appropriate clear, marks the new buffer
    /// fully touched, and then resumes normal diffing.
    pub fn request_clear(&mut self) {
        self.force_clear = true;
    }

    /// Render a desired buffer state into terminal bytes.
    ///
    /// # Parameters
    ///
    /// - `out`: byte buffer receiving escape sequences and glyph bytes.
    /// - `new_buf`: desired cell state. Its touched flags gate which
    ///   rows are considered, except when a force-clear marks all rows.
    ///
    /// # Returns
    ///
    /// `Ok(())` after appending all required bytes and updating tracked
    /// renderer state.
    ///
    /// # Errors
    ///
    /// Propagates errors from `Write` implementations used by low-level
    /// emit helpers. With the current `Vec<u8>` output this is
    /// effectively infallible.
    ///
    /// # Usage notes
    ///
    /// No output is produced when neither the buffer has touched rows nor
    /// a force-clear is pending.
    pub fn render(&mut self, out: &mut Vec<u8>, new_buf: &mut RenderBuffer) -> io::Result<()> {
        let width = new_buf.width();
        let height = new_buf.height();

        if !self.force_clear && !new_buf.has_changes() {
            return Ok(());
        }

        let resized = self.prepare_frame(out, new_buf, width, height)?;
        self.diff_frame(out, new_buf, width, height)?;
        self.finalize_frame(out, new_buf, resized, height)?;

        Ok(())
    }

    /// Render the renderer-owned staging buffer.
    ///
    /// The staging buffer is populated by `sync_front`. This method
    /// temporarily moves it out to avoid mutable aliasing, renders it
    /// through [`Renderer::render`], clears its touched flags, and stores
    /// it back for reuse by the next frame.
    pub(crate) fn render_back(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        // Swap back_buf out so the existing pipeline can borrow it as
        // `new_buf` without aliasing `&mut self`. The placeholder
        // RenderBuffer holds no heap storage (zero rows).
        let mut back = std::mem::replace(&mut self.back_buf, RenderBuffer::new(0, 0));
        let result = self.render(out, &mut back);
        back.clear_touched();
        self.back_buf = back;
        result
    }
}

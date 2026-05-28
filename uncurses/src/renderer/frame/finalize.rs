//! Per-frame epilogue.
//!
//! Runs after the diff pass: snaps the cursor to the bottom-left of
//! the new surface on inline resizes so the next inline write lands
//! where the user expects, then resets the pen (closing any open
//! hyperlink) so the renderer leaves the terminal in a clean state.

use std::io;

use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;

impl Renderer {
    /// Run the frame epilogue: cursor snap on inline resizes plus pen
    /// reset.
    pub(super) fn finalize_frame(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &RenderBuffer,
        resized: bool,
        height: u16,
    ) -> io::Result<()> {
        // Inline mode: after an inline resize, snap the cursor back to
        // the bottom-left of the new surface so the next inline output
        // (or restored shell prompt) starts where the user expects it.
        if resized && !self.fullscreen && height > 0 {
            self.move_to(out, new_buf, height - 1, 0)?;
        }

        // Reset pen + close any open hyperlink to a clean state. Going
        // through update_pen rather than emitting a raw SGR reset means
        // we skip the bytes entirely when the pen is already default,
        // and we correctly emit an OSC 8 close when a link is open.
        self.update_pen(out, None)?;

        Ok(())
    }
}

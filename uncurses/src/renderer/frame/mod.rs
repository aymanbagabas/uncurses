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
//! - [`emit`] – cursor and glyph emission helpers used by
//!   both the diff loop and the rest of the renderer.
//! - [`finalize`] – per-frame epilogue: inline-resize cursor snap and
//!   pen/link reset.

pub(super) mod diff;
pub(super) mod emit;
pub(super) mod finalize;
pub(super) mod prepare;

use std::io;

use crate::ansi::mode::Mode;
use crate::layout::{Position, Size};
use crate::renderer::Renderer;
use crate::renderer::buffer::RenderBuffer;
use crate::renderer::regions::RegionFrame;
use crate::screen::RegionResource;
use std::sync::Arc;

impl Renderer {
    pub(crate) fn begin_frame(&self, out: &mut Vec<u8>, cursor_visible: bool) {
        if self.sync_output {
            Mode::SYNCHRONIZED_OUTPUT.set(out).unwrap();
        } else if cursor_visible {
            Mode::CURSOR_VISIBLE.reset(out).unwrap();
        }
    }

    pub(crate) fn end_frame(&self, out: &mut Vec<u8>, cursor_visible: bool) {
        if self.sync_output {
            Mode::SYNCHRONIZED_OUTPUT.reset(out).unwrap();
        } else if cursor_visible {
            Mode::CURSOR_VISIBLE.set(out).unwrap();
        }
    }

    /// Emit one prevalidated frame through the shared text and native pipeline.
    pub(crate) fn stage_frame(
        &mut self,
        out: &mut Vec<u8>,
        front: &RenderBuffer,
        mut regions: Option<&mut RegionFrame>,
        cursor: Option<Position>,
        cursor_visible: bool,
    ) {
        let size = Size::new(front.width(), front.height());
        self.begin_frame(out, cursor_visible);
        if let Some(frame) = regions.as_deref_mut() {
            self.retire_regions(out, frame, size);
        }
        let upload = regions
            .as_deref()
            .map_or(&[][..], |frame| frame.upload.as_slice());
        if upload.is_empty() {
            self.render_back(out).unwrap();
        } else {
            self.render_back_with_resources(out, upload).unwrap();
        }
        self.draw_regions(out, regions.as_deref(), size);
        if let Some(pos) = cursor {
            if self.regions.is_some() {
                self.move_to_between_frames(out, size, pos.y, pos.x)
                    .unwrap();
            } else {
                self.move_to_resting(out, front, pos.y, pos.x).unwrap();
            }
        }
        self.end_frame(out, cursor_visible);
    }

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
    /// Propagates errors from `Write` implementations used by the emit helpers. With the current `Vec<u8>` output this is
    /// effectively infallible.
    ///
    /// # Usage notes
    ///
    /// No output is produced when neither the buffer has touched rows nor
    /// a force-clear is pending.
    pub fn render(&mut self, out: &mut Vec<u8>, new_buf: &mut RenderBuffer) -> io::Result<()> {
        self.render_with_resources(out, new_buf, &[])
    }

    fn render_with_resources(
        &mut self,
        out: &mut Vec<u8>,
        new_buf: &mut RenderBuffer,
        resources: &[Arc<RegionResource>],
    ) -> io::Result<()> {
        let width = new_buf.width();
        let height = new_buf.height();

        if !self.force_clear && !new_buf.has_changes() {
            for resource in resources {
                out.extend_from_slice(&resource.create);
            }
            return Ok(());
        }

        let resized = self.prepare_frame(out, new_buf, width, height)?;
        // A full terminal clear can delete even resources without placements.
        for resource in resources {
            out.extend_from_slice(&resource.create);
        }
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
        self.render_back_with_resources(out, &[])
    }

    pub(crate) fn render_back_with_resources(
        &mut self,
        out: &mut Vec<u8>,
        resources: &[Arc<RegionResource>],
    ) -> io::Result<()> {
        // Swap back_buf out so the existing pipeline can borrow it as
        // `new_buf` without aliasing `&mut self`. The placeholder
        // RenderBuffer holds no heap storage (zero rows).
        let mut back = std::mem::replace(&mut self.back_buf, RenderBuffer::new(0, 0));
        let result = if resources.is_empty() {
            self.render(out, &mut back)
        } else {
            self.render_with_resources(out, &mut back, resources)
        };
        back.clear_touched();
        self.back_buf = back;
        result
    }
}

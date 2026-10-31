use std::io::{self, Write};

use super::Screen;
use crate::layout::Position;

impl<W: Write> Screen<W> {
    /// Render and release the top `count` inline rows without erasing them.
    ///
    /// Retained rows keep their displayed contents and receive new coordinates.
    /// Releasing every row starts an empty area on the next line.
    /// Counts are clamped to the managed height. Fullscreen screens and zero
    /// counts leave pending cells and output unchanged.
    ///
    /// A render error leaves the rows managed. The rows are released before
    /// the final flush, even if it fails. Use [`Write::flush`] to retry output
    /// without releasing more rows.
    pub fn commit_rows(&mut self, count: u16) -> io::Result<()> {
        let count = count.min(self.height);
        if self.fullscreen || count == 0 {
            return Ok(());
        }
        self.render()?;
        if count == self.height {
            self.stage_move_cursor_to(Position::new(0, self.height - 1));
            self.out_buf.extend_from_slice(b"\r\n");
            self.renderer
                .set_cursor_position(Position::new(0, self.height));
        } else if !self.renderer.cursor_known() || self.renderer.cursor_position().y < count {
            self.stage_move_cursor_to(Position::new(0, count));
        }
        self.renderer.commit_rows(&mut self.front_buf, count);
        self.height -= count;
        if let Some(position) = &mut self.desired_cursor {
            position.y = position.y.saturating_sub(count);
        }
        self.cursor_rested_at = None;
        self.flush()
    }
}

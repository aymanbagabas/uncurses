//! Whole-row emission and physical frame checkpoints.

use std::io;

use crate::ansi;
use crate::cell::Cell;
use crate::layout::{Position, Size};
use crate::renderer::Renderer;
use crate::renderer::placements::Record;
use crate::renderer::state::Cursor;
use crate::style::Style;

pub(super) struct FrameCheckpoint {
    cursor: Cursor,
    pen_unknown: bool,
    width: u16,
    height: u16,
    protected_rows: Vec<bool>,
}

pub(super) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

impl Renderer {
    pub(super) fn validate_row(&self, cells: &[Cell]) -> io::Result<()> {
        let mut x = 0;
        while x < cells.len() {
            let cell = &cells[x];
            let width = cell.width() as usize;
            let end = x + width;
            if width == 0
                || end > cells.len()
                || cell.content().chars().any(char::is_control)
                || (!cell.content().is_empty()
                    && ![false, true].into_iter().any(|wide| {
                        ansi::text::string_width(cell.content().as_bytes(), self.width_mode, wide)
                            == width
                    }))
                || cells[x + 1..end]
                    .iter()
                    .any(|c| !c.is_continuation() || !c.content().is_empty())
            {
                return Err(invalid(
                    "placement row contains an invalid cell or wide-cell span",
                ));
            }
            x = end;
        }
        Ok(())
    }

    pub(super) fn write_placement(
        &mut self,
        out: &mut Vec<u8>,
        size: Size,
        pos: Position,
        bytes: &[u8],
    ) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.reset_pen(out)?;
        self.move_to_between_frames(
            out,
            Size::new(
                size.width.max(self.last_width),
                size.height.max(self.last_height),
            ),
            pos.y,
            pos.x,
        )?;
        out.extend_from_slice(b"\x1b7");
        self.update_pen(out, Some(&Cell::BLANK))?;
        out.extend_from_slice(bytes);
        // Opaque bytes can change SGR or OSC 8 without updating the tracked pen.
        out.extend_from_slice(b"\x1b]8;;\x1b\\\x1b[m\x1b8");
        self.cur.set_style(Style::default());
        Ok(())
    }

    pub(super) fn draw_overlay(
        &mut self,
        out: &mut Vec<u8>,
        row: u16,
        cells: &[Cell],
        placements: &[Record],
    ) -> io::Result<()> {
        // The backing row already contains text outside the payload rectangles.
        // Replay whole glyphs, including those whose continuations overlap.
        let covered = |x: usize| {
            placements.iter().any(|record| {
                record.options.cells.is_none()
                    && row >= record.area.y
                    && row < record.area.bottom()
                    && x < usize::from(record.area.right())
                    && x + usize::from(cells[x].width()) > usize::from(record.area.x)
            })
        };
        let mut x = 0usize;
        while x < cells.len() {
            if cells[x].is_placement() || cells[x].is_continuation() || !covered(x) {
                x += 1;
                continue;
            }
            self.move_to_between_frames(
                out,
                Size::new(self.last_width, self.last_height),
                row,
                x as u16,
            )?;
            ansi::mode::Mode::AUTO_WRAP.reset(out)?;
            while x < cells.len() && !cells[x].is_placement() && covered(x) {
                let cell = &cells[x];
                if cell.is_continuation() {
                    x += 1;
                    continue;
                }
                self.update_pen(out, Some(cell))?;
                out.extend_from_slice(cell.draw_bytes().as_ref());
                x += usize::from(cell.width());
            }
            ansi::mode::Mode::AUTO_WRAP.set(out)?;
            self.reanchor_to_row_start(out);
        }
        Ok(())
    }

    pub(super) fn checkpoint_frame(&self) -> FrameCheckpoint {
        FrameCheckpoint {
            cursor: self.cur.clone(),
            pen_unknown: self.pen_unknown,
            width: self.last_width,
            height: self.last_height,
            protected_rows: self.protected_rows.clone(),
        }
    }

    pub(super) fn restore_staged_frame(&mut self, checkpoint: FrameCheckpoint) {
        self.cur = checkpoint.cursor;
        self.pen_unknown = checkpoint.pen_unknown;
        self.last_width = checkpoint.width;
        self.last_height = checkpoint.height;
        self.protected_rows = checkpoint.protected_rows;
        // Speculative cell changes need a repaint, not a full-grid rollback.
        self.cur_buf = None;
        self.invalid_rows.clear();
        self.old_hashes.clear();
        self.new_hashes.clear();
    }

    pub(super) fn protect_failed_frame(&mut self, checkpoint: FrameCheckpoint) {
        // A partial write can leave either the old or attempted inline tail.
        self.last_height = self.last_height.max(checkpoint.height);
        self.protected_rows.resize(
            self.protected_rows
                .len()
                .max(checkpoint.protected_rows.len()),
            false,
        );
        for (row, protected) in self.protected_rows.iter_mut().enumerate() {
            *protected |= checkpoint.protected_rows.get(row) == Some(&true);
        }
    }

    pub(crate) fn row_protected(&self, row: u16) -> bool {
        self.protected_rows.get(row as usize) == Some(&true)
    }

    pub(super) fn paint_complete_row(
        &mut self,
        out: &mut Vec<u8>,
        row: u16,
        cells: &[Cell],
    ) -> io::Result<()> {
        if cells.is_empty() {
            return Ok(());
        }
        self.move_to_between_frames(out, Size::new(self.last_width, self.last_height), row, 0)?;
        self.reset_pen(out)?;
        // Erase any tail left by a cluster the terminal measures differently.
        out.extend_from_slice(b"\x1b[2K");
        ansi::mode::Mode::AUTO_WRAP.reset(out)?;
        for cell in cells.iter().filter(|cell| !cell.is_continuation()) {
            self.update_pen(out, Some(cell))?;
            out.extend_from_slice(cell.draw_bytes().as_ref());
        }
        ansi::mode::Mode::AUTO_WRAP.set(out)?;
        self.reanchor_to_row_start(out);
        Ok(())
    }

    pub(super) fn record_row(&mut self, row: u16, cells: &[Cell]) {
        if let Some(line) = self.cur_buf.as_mut().and_then(|buf| buf.line_mut(row)) {
            line.clone_from_slice(cells);
        }
        self.invalid_rows[row as usize] = false;
    }
}

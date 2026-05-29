use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell as RtCell;
use ratatui::layout::{Position as RtPosition, Size as RtSize};
use uncurses::buffer::{Bounded, Surface, SurfaceMut};
use uncurses::cell::Cell as CzCell;
use uncurses::screen::Screen;
use uncurses::terminal::{size as size_mod, tty};

use crate::convert::cell_from_ratatui;

/// A `ratatui` [`Backend`](ratatui::backend::Backend) that renders through a
/// [`uncurses::screen::Screen`].
///
/// Mirrors the conventions of other ratatui backends: side-effecting methods
/// (`hide_cursor`, `show_cursor`, `set_cursor_position`, `clear`,
/// `clear_region`) flush immediately, while `draw` only updates the back
/// buffer and defers I/O to the next `flush`.
///
/// # Supported viewports
///
/// Only [`ratatui::Viewport::Fullscreen`] and [`ratatui::Viewport::Fixed`]
/// are supported. [`ratatui::Viewport::Inline`] requires the backend to
/// report the real cursor position from the controlling terminal so
/// ratatui can anchor the inline area; that is not implemented here, and
/// constructing a `Terminal` with an inline viewport will fail with
/// [`io::ErrorKind::Unsupported`] from [`Backend::get_cursor_position`].
pub struct UncursesBackend<W: Write> {
    screen: Screen<W>,
}

impl<W: Write> UncursesBackend<W> {
    /// Wrap a `Screen` for use as a ratatui backend.
    pub fn new(screen: Screen<W>) -> Self {
        Self { screen }
    }

    /// Borrow the underlying screen.
    pub fn screen(&self) -> &Screen<W> {
        &self.screen
    }

    /// Borrow the underlying screen mutably.
    pub fn screen_mut(&mut self) -> &mut Screen<W> {
        &mut self.screen
    }

    /// Consume the backend and return the wrapped screen.
    pub fn into_inner(self) -> Screen<W> {
        self.screen
    }
}

impl<W: Write> Write for UncursesBackend<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.screen.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.screen)
    }
}

impl<W: Write> Backend for UncursesBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a RtCell)>,
    {
        for (x, y, rc) in content {
            let cell = cell_from_ratatui(rc);
            self.screen.set_cell((x, y), &cell);
        }
        self.screen.render()
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.screen.set_cursor_visible(false)?;
        Write::flush(&mut self.screen)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.screen.set_cursor_visible(true)?;
        Write::flush(&mut self.screen)
    }

    fn get_cursor_position(&mut self) -> io::Result<RtPosition> {
        // Reporting the real cursor position requires a DSR/CPR round-trip
        // through the controlling terminal. That path is not implemented,
        // and returning the locally-cached position would silently break
        // any caller that depends on a true reading (e.g. ratatui's
        // inline viewport anchor). Surface the limitation as Unsupported
        // so Terminal::with_options(Viewport::Inline(_)) fails cleanly at
        // construction time.
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "get_cursor_position is not supported by UncursesBackend; \
             only fullscreen and fixed viewports are supported",
        ))
    }

    fn set_cursor_position<P: Into<RtPosition>>(&mut self, position: P) -> io::Result<()> {
        let p = position.into();
        self.screen.set_cursor_position(p.x, p.y)?;
        Write::flush(&mut self.screen)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    /// Blank only the cells covered by `clear_type` in the screen's
    /// staging buffer and restore the cursor to its prior position. The
    /// renderer's diff emits whatever is needed to bring the wire in
    /// sync on the next [`Backend::flush`].
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        let w = self.screen.width();
        let h = self.screen.height();
        let cursor = self.screen.cursor_position();
        if w == 0 || h == 0 {
            return Ok(());
        }
        let region = match clear_type {
            ClearType::All => Some(uncurses::layout::Rect::new(0, 0, w, h)),
            ClearType::AfterCursor => {
                if cursor.y < h {
                    let tail_x = cursor.x.min(w);
                    self.screen.fill_rect(
                        uncurses::layout::Rect::new(tail_x, cursor.y, w - tail_x, 1),
                        &CzCell::BLANK,
                    );
                }
                (cursor.y + 1 < h)
                    .then(|| uncurses::layout::Rect::new(0, cursor.y + 1, w, h - cursor.y - 1))
            }
            ClearType::BeforeCursor => {
                if cursor.y > 0 {
                    self.screen.fill_rect(
                        uncurses::layout::Rect::new(0, 0, w, cursor.y),
                        &CzCell::BLANK,
                    );
                }
                (cursor.y < h).then(|| {
                    let head_w = (cursor.x.min(w).saturating_add(1)).min(w);
                    uncurses::layout::Rect::new(0, cursor.y, head_w, 1)
                })
            }
            ClearType::CurrentLine => {
                (cursor.y < h).then(|| uncurses::layout::Rect::new(0, cursor.y, w, 1))
            }
            ClearType::UntilNewLine => (cursor.y < h && cursor.x < w)
                .then(|| uncurses::layout::Rect::new(cursor.x, cursor.y, w - cursor.x, 1)),
        };
        if let Some(region) = region {
            self.screen.fill_rect(region, &CzCell::BLANK);
        }
        // Restore the cursor to where it was before the blank: ratatui's
        // Backend contract guarantees clear preserves the cursor position.
        self.screen.set_cursor_position(cursor.x, cursor.y)?;
        // Push the staged blanks to the wire so the clear takes effect
        // before this call returns, matching the immediate-clear contract.
        self.screen.render()?;
        Write::flush(&mut self.screen)
    }

    fn size(&self) -> io::Result<RtSize> {
        let b = self.screen.bounds();
        Ok(RtSize {
            width: b.width(),
            height: b.height(),
        })
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let b = self.screen.bounds();
        let (_, out) = tty::open_tty()?;
        let w = size_mod::get_window_size(&out).ok();
        Ok(WindowSize {
            columns_rows: RtSize {
                width: b.width(),
                height: b.height(),
            },
            pixels: RtSize {
                width: w.as_ref().map(|w| w.xpixel).unwrap_or(0),
                height: w.as_ref().map(|w| w.ypixel).unwrap_or(0),
            },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.screen)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        if n == 0 {
            return Ok(());
        }
        // `insert_above` splits its argument on `\n`, so `m` newlines
        // produce `m + 1` lines. We want `n` blank lines.
        let content = "\n".repeat(n.saturating_sub(1) as usize);
        self.screen.insert_above(&content)?;
        Write::flush(&mut self.screen)
    }

    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        // When the region includes row 0, the rows scrolled off the top must
        // be preserved in the terminal's scrollback (per the backend contract:
        // see `ratatui_core::backend::Backend::scroll_region_up`). Route those
        // rows through `Screen::insert_above` so the host terminal commits
        // them to its scrollback buffer, and flush so they reach the wire
        // before subsequent draws move the cursor.
        if region.start == 0 && amount > 0 && region.end > 0 {
            let n = amount.min(region.end);
            let content = self.snapshot_rows_text(0..n);
            self.screen.insert_above(&content)?;
            Write::flush(&mut self.screen)?;
        }
        // The shift itself only mutates the staged buffer; the SU/DL bytes
        // are emitted later by the renderer when the next `draw` flushes.
        self.screen
            .delete_lines(region.start, amount, region.end, &CzCell::BLANK);
        Ok(())
    }

    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        // Mutates the staged buffer only; the SD/IL bytes are emitted later
        // by the renderer when the next `draw` flushes.
        self.screen
            .insert_lines(region.start, amount, region.end, &CzCell::BLANK);
        Ok(())
    }
}

impl<W: Write> UncursesBackend<W> {
    /// Read rows `[rows.start, rows.end)` from the screen's staged buffer and
    /// join them with `\n`, trimming trailing blanks per line. Continuation
    /// cells of wide characters are skipped to avoid duplicating glyphs.
    fn snapshot_rows_text(&self, rows: std::ops::Range<u16>) -> String {
        let b = self.screen.bounds();
        let width = b.width();
        let mut out = String::new();
        for (i, y) in rows.clone().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let mut line = String::new();
            for x in 0..width {
                if let Some(cell) = self.screen.cell((x, y).into())
                    && cell.width() > 0
                {
                    line.push_str(cell.content());
                }
            }
            let trimmed = line.trim_end();
            out.push_str(trimmed);
        }
        out
    }
}

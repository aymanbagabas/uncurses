//! Terminal-lifecycle operations for [`Screen`] — `reset` / `restore`
//! tear down and re-apply held modes around a shell handoff, and
//! `insert_above` injects content into the scrollback above the screen.

use std::io::{self, Write};

use crate::ansi::{self, cursor, mode};

use super::Screen;

impl<W: Write> Screen<W> {
    /// Disable every non-default terminal state currently held in
    /// `self.state` so the terminal is returned to a clean baseline,
    /// suitable for handing control back to the shell (e.g. before
    /// suspending the process or exec-ing a child). Pure write — does
    /// not mutate `self.state`, so a subsequent [`Screen::restore`]
    /// can re-apply the same modes verbatim.
    pub fn reset(&mut self) -> io::Result<()> {
        // Walk to the bottom of the *last rendered* surface before any
        // mode teardown. Use the renderer's last-render height rather
        // than the live screen height: a terminal that grew between
        // the last render and quit hasn't drawn anything below the
        // old bottom row, so addressing the new (taller) bottom would
        // pull the post-quit cursor far below where the user started
        // on terminals where DECRST 1049's saved-cursor restore is
        // unreliable across resizes. Done in both inline and
        // alt-screen modes: in inline mode we need it so a returning
        // shell prompt prints below the surface; in alt-screen mode,
        // terminals that don't honor 1049's saved cursor still land
        // at a sensible position rather than wherever the last render
        // left the cursor.
        let (_, last_height) = self.renderer.last_size();
        if last_height > 0 {
            self.renderer
                .move_to(&mut self.buf, &self.front_buf, last_height - 1, 0)?;
        }
        if !self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.set(&mut self.buf)?;
        }
        if self.state.cursor_style != cursor::CursorStyle::Default {
            cursor::write_cursor_style(&mut self.buf, cursor::CursorStyle::Default)?;
        }
        if self.state.bracketed_paste {
            mode::Mode::BRACKETED_PASTE.reset(&mut self.buf)?;
        }
        if self.state.focus_events {
            mode::Mode::FOCUS.reset(&mut self.buf)?;
        }
        if self.state.mouse_mode != mode::MouseMode::None {
            mode::write_disable_mouse(
                &mut self.buf,
                self.state.mouse_mode,
                self.state.mouse_encoding,
            )?;
        }
        if self.state.alt_screen {
            mode::Mode::ALT_SCREEN_SAVE_CURSOR.reset(&mut self.buf)?;
            self.renderer.restore_cursor();
        }
        if self.state.grapheme_clusters {
            mode::Mode::UNICODE_CORE.reset(&mut self.buf)?;
        }
        if self.state.color_scheme_updates {
            mode::Mode::LIGHT_DARK.reset(&mut self.buf)?;
        }
        if self.state.title.is_some() {
            ansi::write_window_title(&mut self.buf, "")?;
        }
        Ok(())
    }

    /// Re-emit every non-default mode held in `self.state` to `w`.
    /// Pairs with [`Screen::reset`] for any scenario where the
    /// terminal was temporarily handed back to the shell. Pure write —
    /// does not mutate `self.state`. Call [`Screen::invalidate`]
    /// afterwards if the screen contents also need to be repainted.
    pub fn restore(&mut self) -> io::Result<()> {
        if self.state.alt_screen {
            self.renderer.save_cursor();
            mode::Mode::ALT_SCREEN_SAVE_CURSOR.set(&mut self.buf)?;
        }
        if self.state.grapheme_clusters {
            mode::Mode::UNICODE_CORE.set(&mut self.buf)?;
        }
        if self.state.color_scheme_updates {
            mode::Mode::LIGHT_DARK.set(&mut self.buf)?;
        }
        if !self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.reset(&mut self.buf)?;
        }
        if self.state.cursor_style != cursor::CursorStyle::Default {
            cursor::write_cursor_style(&mut self.buf, self.state.cursor_style)?;
        }
        if self.state.bracketed_paste {
            mode::Mode::BRACKETED_PASTE.set(&mut self.buf)?;
        }
        if self.state.focus_events {
            mode::Mode::FOCUS.set(&mut self.buf)?;
        }
        if self.state.mouse_mode != mode::MouseMode::None {
            mode::write_enable_mouse(
                &mut self.buf,
                self.state.mouse_mode,
                self.state.mouse_encoding,
            )?;
        }
        if let Some(ref title) = self.state.title {
            ansi::write_window_title(&mut self.buf, title)?;
        }
        Ok(())
    }

    /// Insert `content` above the screen, scrolling the screen down to
    /// make room. Long lines that exceed the screen width are accounted
    /// for so the screen state is preserved exactly.
    ///
    /// In inline mode this pushes the inserted lines into the terminal's
    /// scrollback. In alt screen mode the inserted lines go into the
    /// alt screen's hidden scrollback, where they will not be visible
    /// but will also not corrupt the rendered frame.
    ///
    /// Only writes — does not flush. Forces a full redraw on the next
    /// [`Screen::render`].
    pub fn insert_above(&mut self, content: &str) -> io::Result<()> {
        if content.is_empty() {
            return Ok(());
        }

        let width = self.width;
        let height = self.height;
        let y = self.renderer.cursor_position().y;

        self.buf.write_all(b"\r")?;
        let down = height.saturating_sub(y).saturating_sub(1);
        if down > 0 {
            cursor::write_cud(&mut self.buf, down)?;
        }

        let lines: Vec<&str> = content.split('\n').collect();
        let mut offset: u16 = lines.len() as u16;
        for line in &lines {
            let lw =
                ansi::text::string_width(line.as_bytes(), self.width_mode(), self.eaw_wide) as u16;
            if let Some(n) = lw.checked_div(width) {
                offset = offset.saturating_add(n);
            }
        }

        for _ in 0..offset {
            self.buf.write_all(b"\n")?;
        }

        let up = offset.saturating_add(height).saturating_sub(1);
        if up > 0 {
            cursor::write_cuu(&mut self.buf, up)?;
        }
        ansi::screen::write_insert_lines(&mut self.buf, offset)?;
        for line in &lines {
            self.buf.write_all(line.as_bytes())?;
            self.buf.write_all(ansi::screen::ERASE_LINE_RIGHT)?;
            self.buf.write_all(b"\r\n")?;
        }

        self.renderer
            .set_cursor_position(crate::Position { y: 0, x: 0 });
        self.renderer.request_clear();
        Ok(())
    }
}

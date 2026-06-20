//! Terminal-lifecycle operations for [`Canvas`] — `reset` / `restore`
//! tear down and re-apply held modes around a shell handoff, and
//! `insert_above` injects content into the scrollback above the screen.

use std::io::Write;

use crate::ansi::{self, cursor, kitty, mode};
use crate::text::TextSurface;

use super::Canvas;

impl<W: Write> Canvas<W> {
    /// Disable every non-default terminal state currently held in
    /// `self.state` so the terminal is returned to a clean baseline,
    /// suitable for handing control back to the shell (e.g. before
    /// suspending the process or exec-ing a child). Pure write — does
    /// not mutate `self.state`, so a subsequent [`Canvas::restore`]
    /// can re-apply the same modes verbatim.
    ///
    /// Only stages into the buffer, so it is infallible; the bytes reach
    /// the terminal on the next [`Canvas::flush`].
    pub fn reset(&mut self) {
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
                .move_to(&mut self.buf, &self.front_buf, last_height - 1, 0)
                .unwrap();
        }
        if !self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.set(&mut self.buf).unwrap();
        }
        // Clear the alt screen's kitty keyboard frame *before* leaving
        // the alt screen — the stack is per-screen-buffer, so the
        // clear must be issued while alt is still active.
        if self.state.alt_screen && !self.state.kitty_keyboard.is_empty() {
            kitty::write_set_kitty_keyboard(
                &mut self.buf,
                kitty::KittyKeyboardFlags::NONE,
                kitty::KittyKeyboardMode::Set,
            )
            .unwrap();
        }
        if self.state.alt_screen {
            mode::Mode::ALT_SCREEN_SAVE_CURSOR
                .reset(&mut self.buf)
                .unwrap();
            self.renderer.restore_cursor();
        }
        // Now on the main screen — clear its frame too.
        if !self.state.kitty_keyboard.is_empty() {
            kitty::write_set_kitty_keyboard(
                &mut self.buf,
                kitty::KittyKeyboardFlags::NONE,
                kitty::KittyKeyboardMode::Set,
            )
            .unwrap();
        }
        if self.state.grapheme_clusters {
            mode::Mode::UNICODE_CORE.reset(&mut self.buf).unwrap();
        }
    }

    /// Re-emit every non-default mode held in `self.state` to `w`.
    /// Pairs with [`Canvas::reset`] for any scenario where the
    /// terminal was temporarily handed back to the shell. Pure write —
    /// does not mutate `self.state`. Call [`Canvas::invalidate`]
    /// afterwards if the screen contents also need to be repainted.
    ///
    /// Only stages into the buffer, so it is infallible; the bytes reach
    /// the terminal on the next [`Canvas::flush`].
    pub fn restore(&mut self) {
        // Re-apply the desired kitty keyboard flags on the main
        // screen *before* entering the alt screen — the stack is
        // per-buffer, so a set targeting main must happen while main
        // is active.
        if !self.state.kitty_keyboard.is_empty() {
            kitty::write_set_kitty_keyboard(
                &mut self.buf,
                self.state.kitty_keyboard,
                kitty::KittyKeyboardMode::Set,
            )
            .unwrap();
        }
        if self.state.alt_screen {
            self.renderer.save_cursor();
            mode::Mode::ALT_SCREEN_SAVE_CURSOR
                .set(&mut self.buf)
                .unwrap();
        }
        // Now on the alt screen (if alt was active) — re-apply on
        // the alt buffer too, since its stack is independent.
        if self.state.alt_screen && !self.state.kitty_keyboard.is_empty() {
            kitty::write_set_kitty_keyboard(
                &mut self.buf,
                self.state.kitty_keyboard,
                kitty::KittyKeyboardMode::Set,
            )
            .unwrap();
        }
        if self.state.grapheme_clusters {
            mode::Mode::UNICODE_CORE.set(&mut self.buf).unwrap();
        }
        if !self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.reset(&mut self.buf).unwrap();
        }
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
    /// [`Canvas::render`].
    ///
    /// Only stages into the buffer, so it is infallible; the bytes reach
    /// the terminal on the next [`Canvas::flush`].
    pub fn insert_above(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }

        let width = self.width;
        let height = self.height;
        let y = self.renderer.cursor_position().y;

        self.buf.write_all(b"\r").unwrap();
        let down = height.saturating_sub(y).saturating_sub(1);
        if down > 0 {
            cursor::write_cud(&mut self.buf, down).unwrap();
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
            self.buf.write_all(b"\n").unwrap();
        }

        let up = offset.saturating_add(height).saturating_sub(1);
        if up > 0 {
            cursor::write_cuu(&mut self.buf, up).unwrap();
        }
        ansi::screen::write_insert_lines(&mut self.buf, offset).unwrap();
        for line in &lines {
            self.buf.write_all(line.as_bytes()).unwrap();
            self.buf.write_all(ansi::screen::ERASE_LINE_RIGHT).unwrap();
            self.buf.write_all(b"\r\n").unwrap();
        }

        self.renderer
            .set_cursor_position(crate::layout::Position { y: 0, x: 0 });
        self.renderer.request_clear();
    }
}

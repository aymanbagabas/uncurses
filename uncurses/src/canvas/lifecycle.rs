//! Terminal-lifecycle operations for [`Canvas`] — `reset` / `restore`
//! tear down and re-apply held modes around a shell handoff, and
//! `insert_above` injects content into the scrollback above the screen.

use std::io::Write;

use crate::ansi::{self, cursor, kitty, mode};
use crate::text::TextSurface;

use super::Canvas;

impl<W: Write> Canvas<W> {
    /// Stage terminal-state teardown for a shell handoff.
    ///
    /// # Behavior
    ///
    /// Disables every non-default terminal mode tracked in canvas state:
    /// cursor visibility, alternate screen, Kitty keyboard enhancements,
    /// and Unicode core mode. Before tearing modes down, moves to the
    /// bottom of the last rendered surface so inline shell output resumes
    /// below the application area.
    ///
    /// This is a pure write to the canvas staging buffer: it does not
    /// mutate `self.state`, so a later [`Canvas::restore`] can re-apply
    /// the same tracked modes verbatim.
    ///
    /// # Panics
    ///
    /// Never panics; bytes are staged into memory.
    ///
    /// Only stages into the buffer, so it is infallible; the bytes reach
    /// the terminal on the next [`std::io::Write::flush`].
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

    /// Stage terminal-state restoration after a shell handoff.
    ///
    /// # Behavior
    ///
    /// Re-emits every non-default mode tracked in canvas state, including
    /// Kitty keyboard flags on the appropriate screen buffer, alternate
    /// screen, Unicode core mode, and cursor visibility. Pairs with
    /// [`Canvas::reset`] for suspend/resume or child-process handoffs.
    ///
    /// This is a pure write to the canvas staging buffer and does not
    /// mutate `self.state`.
    ///
    /// # Panics
    ///
    /// Never panics; bytes are staged into memory.
    ///
    /// # Usage notes
    ///
    /// Call [`Canvas::invalidate`] afterwards if the screen contents may
    /// have changed while the terminal was handed away.
    ///
    /// Only stages into the buffer, so it is infallible; the bytes reach
    /// the terminal on the next [`std::io::Write::flush`].
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

    /// Insert text above the managed surface.
    ///
    /// # Parameters
    ///
    /// - `content`: UTF-8 text to write above the canvas. An empty string
    ///   is a no-op.
    ///
    /// # Behavior
    ///
    /// Moves to the top of the managed area, creates enough physical
    /// lines for `content` (including wrapped long lines), inserts those
    /// lines above the canvas, writes the content with line erases, then
    /// requests a full redraw for the next render.
    ///
    /// In inline mode this pushes the inserted lines into the terminal's
    /// scrollback. In alt screen mode the inserted lines go into the
    /// alt screen's hidden scrollback, where they will not be visible
    /// but will also not corrupt the rendered frame.
    ///
    /// # Panics
    ///
    /// Never panics; bytes are staged into memory.
    ///
    /// # Usage notes
    ///
    /// Only writes — does not flush. Forces a full redraw on the next
    /// [`Canvas::render`].
    ///
    /// Only stages into the buffer, so it is infallible; the bytes reach
    /// the terminal on the next [`std::io::Write::flush`].
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

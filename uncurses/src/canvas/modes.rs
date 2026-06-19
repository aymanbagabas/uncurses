//! Render-coupled mode toggles for [`Canvas`] — alt screen, cursor
//! visibility, synchronized output, and grapheme clusters
//! (DEC 2027).
//!
//! These setters only stage escape bytes into the screen's in-memory
//! buffer, so they are infallible and return `()`. The bytes reach the
//! terminal when the caller invokes [`Canvas::flush`] (or
//! [`Canvas::present`]), which is the sole fallible I/O boundary.

use std::io::Write;

use crate::ansi::{kitty, mode};

use super::Canvas;

impl<W: Write> Canvas<W> {
    /// Toggle alternate screen mode. Entering saves the cursor and
    /// switches to the alternate buffer; exiting restores both. No-op
    /// when already in the requested state.
    pub fn set_alt_screen(&mut self, alt_screen: bool) {
        if self.state.alt_screen == alt_screen {
            return;
        }
        if alt_screen {
            self.renderer.save_cursor();
            mode::Mode::ALT_SCREEN_SAVE_CURSOR
                .set(&mut self.buf)
                .unwrap();
            self.state.alt_screen = true;
            self.renderer.set_fullscreen(true);
            self.renderer.set_relative_cursor(false);
            self.renderer.request_clear();
        } else {
            mode::Mode::ALT_SCREEN_SAVE_CURSOR
                .reset(&mut self.buf)
                .unwrap();
            self.state.alt_screen = false;
            self.renderer.set_fullscreen(false);
            self.renderer.set_relative_cursor(true);
            self.renderer.restore_cursor();
        }
        // The kitty keyboard stack is per-screen-buffer; re-apply the
        // tracked flags onto the buffer we just switched into so the
        // user-facing flag set is screen-agnostic.
        if !self.state.kitty_keyboard.is_empty() {
            kitty::write_set_kitty_keyboard(
                &mut self.buf,
                self.state.kitty_keyboard,
                kitty::KittyKeyboardMode::Set,
            )
            .unwrap();
        }
    }

    /// Set cursor visibility.
    pub fn set_cursor_visible(&mut self, visible: bool) {
        if self.state.cursor_visible != visible {
            if visible {
                mode::Mode::CURSOR_VISIBLE.set(&mut self.buf).unwrap();
            } else {
                mode::Mode::CURSOR_VISIBLE.reset(&mut self.buf).unwrap();
            }
            self.state.cursor_visible = visible;
        }
    }

    /// Set synchronized updates.
    pub fn set_sync_updates(&mut self, enable: bool) {
        self.state.sync_updates = enable;
    }

    /// Enable or disable Unicode core / grapheme-cluster mode
    /// (DEC private mode 2027). When enabled, [`Canvas::set_str`]
    /// and [`Canvas::insert_above`] calculate cell widths per grapheme
    /// cluster (UTS-29 + emoji presentation rules). When disabled,
    /// widths fall back to per-codepoint wcwidth-style.
    pub fn set_grapheme_clusters(&mut self, enable: bool) {
        if self.state.grapheme_clusters != enable {
            if enable {
                mode::Mode::UNICODE_CORE.set(&mut self.buf).unwrap();
            } else {
                mode::Mode::UNICODE_CORE.reset(&mut self.buf).unwrap();
            }
            self.state.grapheme_clusters = enable;
        }
    }
}

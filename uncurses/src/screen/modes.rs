//! Terminal-mode toggles for [`Screen`] — alt screen, cursor, mouse,
//! bracketed paste, focus reporting, synchronized output, grapheme
//! clusters (DEC 2027), and window title.

use std::io::{self, Write};

use crate::ansi::{self, cursor, kitty, mode};

use super::Screen;

impl<W: Write> Screen<W> {
    /// Toggle alternate screen mode. Entering saves the cursor and
    /// switches to the alternate buffer; exiting restores both. No-op
    /// when already in the requested state.
    pub fn set_alt_screen(&mut self, alt_screen: bool) -> io::Result<()> {
        if self.state.alt_screen == alt_screen {
            return Ok(());
        }
        if alt_screen {
            self.renderer.save_cursor();
            mode::Mode::ALT_SCREEN_SAVE_CURSOR.set(&mut self.buf)?;
            self.state.alt_screen = true;
            self.renderer.set_fullscreen(true);
            self.renderer.set_relative_cursor(false);
            self.renderer.request_clear();
        } else {
            mode::Mode::ALT_SCREEN_SAVE_CURSOR.reset(&mut self.buf)?;
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
            )?;
        }
        Ok(())
    }

    /// Set cursor visibility.
    pub fn set_cursor_visible(&mut self, visible: bool) -> io::Result<()> {
        if self.state.cursor_visible != visible {
            if visible {
                mode::Mode::CURSOR_VISIBLE.set(&mut self.buf)?;
            } else {
                mode::Mode::CURSOR_VISIBLE.reset(&mut self.buf)?;
            }
            self.state.cursor_visible = visible;
        }
        Ok(())
    }

    /// Set cursor style.
    pub fn set_cursor_style(&mut self, style: cursor::CursorStyle) -> io::Result<()> {
        if self.state.cursor_style != style {
            cursor::write_cursor_style(&mut self.buf, style)?;
            self.state.cursor_style = style;
        }
        Ok(())
    }

    /// Enable/disable bracketed paste mode.
    pub fn set_bracketed_paste(&mut self, enable: bool) -> io::Result<()> {
        if self.state.bracketed_paste != enable {
            if enable {
                mode::Mode::BRACKETED_PASTE.set(&mut self.buf)?;
            } else {
                mode::Mode::BRACKETED_PASTE.reset(&mut self.buf)?;
            }
            self.state.bracketed_paste = enable;
        }
        Ok(())
    }

    /// Enable/disable focus in/out reporting.
    pub fn set_focus_events(&mut self, enable: bool) -> io::Result<()> {
        if self.state.focus_events != enable {
            if enable {
                mode::Mode::FOCUS.set(&mut self.buf)?;
            } else {
                mode::Mode::FOCUS.reset(&mut self.buf)?;
            }
            self.state.focus_events = enable;
        }
        Ok(())
    }

    /// Set mouse tracking mode.
    pub fn set_mouse_mode(
        &mut self,
        mouse_mode: mode::MouseMode,
        encoding: mode::MouseEncoding,
    ) -> io::Result<()> {
        // Disable old mode
        if self.state.mouse_mode != mode::MouseMode::None {
            mode::write_disable_mouse(
                &mut self.buf,
                self.state.mouse_mode,
                self.state.mouse_encoding,
            )?;
        }
        // Enable new mode
        if mouse_mode != mode::MouseMode::None {
            mode::write_enable_mouse(&mut self.buf, mouse_mode, encoding)?;
        }
        self.state.mouse_mode = mouse_mode;
        self.state.mouse_encoding = encoding;
        Ok(())
    }

    /// Set synchronized updates.
    pub fn set_sync_updates(&mut self, enable: bool) {
        self.state.sync_updates = enable;
    }

    /// Enable or disable Unicode core / grapheme-cluster mode
    /// (DEC private mode 2027). When enabled, [`Screen::set_str`]
    /// and [`Screen::insert_above`] calculate cell widths per grapheme
    /// cluster (UTS-29 + emoji presentation rules). When disabled,
    /// widths fall back to per-codepoint wcwidth-style.
    pub fn set_grapheme_clusters(&mut self, enable: bool) -> io::Result<()> {
        if self.state.grapheme_clusters != enable {
            if enable {
                mode::Mode::UNICODE_CORE.set(&mut self.buf)?;
            } else {
                mode::Mode::UNICODE_CORE.reset(&mut self.buf)?;
            }
            self.state.grapheme_clusters = enable;
        }
        Ok(())
    }

    /// Enable or disable color scheme update notifications
    /// (DEC private mode 2031). When enabled, the terminal sends a
    /// `CSI ? 997 ; {1|2} n` report whenever the user or operating
    /// system switches between dark and light themes; these surface as
    /// [`Event::DarkColorScheme`] / [`Event::LightColorScheme`].
    ///
    /// [`Event::DarkColorScheme`]: crate::event::Event::DarkColorScheme
    /// [`Event::LightColorScheme`]: crate::event::Event::LightColorScheme
    pub fn set_color_scheme_updates(&mut self, enable: bool) -> io::Result<()> {
        if self.state.color_scheme_updates != enable {
            if enable {
                mode::Mode::LIGHT_DARK.set(&mut self.buf)?;
            } else {
                mode::Mode::LIGHT_DARK.reset(&mut self.buf)?;
            }
            self.state.color_scheme_updates = enable;
        }
        Ok(())
    }

    /// Whether color scheme update notifications (DEC 2031) are enabled.
    pub fn color_scheme_updates(&self) -> bool {
        self.state.color_scheme_updates
    }

    /// Set window title.
    pub fn set_title(&mut self, title: &str) -> io::Result<()> {
        ansi::write_window_title(&mut self.buf, title)?;
        self.state.title = Some(title.to_string());
        Ok(())
    }
}

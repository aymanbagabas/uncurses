//! Non-render terminal/input mode toggles for the [`Screen`] facade —
//! cursor style, mouse tracking, bracketed paste, focus reporting,
//! color-scheme update reports, in-band resize reports, window title,
//! and the default foreground/background/cursor colors.
//!
//! Each setter stages its escape bytes through the owned [`Canvas`] (which
//! buffers them alongside frame output), so they are infallible and return
//! `()`. The bytes reach the terminal on the next
//! [`Screen::flush`](super::Screen::flush) or
//! [`Screen::present`](super::Screen::present).
//!
//! [`Screen`]: super::Screen
//! [`Canvas`]: crate::canvas::Canvas

use std::io::Write;

use crate::ansi::{self, background, cursor, mode};
use crate::color::Color;
use crate::event::source::Input;

use super::Screen;

impl<I: Input, O: Write> Screen<I, O> {
    /// Set the cursor style (`DECSCUSR`).
    pub fn set_cursor_style(&mut self, style: cursor::CursorStyle) {
        if self.modes.cursor_style != style {
            cursor::write_cursor_style(&mut self.canvas, style).unwrap();
            self.modes.cursor_style = style;
        }
    }

    /// Set mouse tracking mode and encoding.
    pub fn set_mouse_mode(&mut self, mouse_mode: mode::MouseMode, encoding: mode::MouseEncoding) {
        if self.modes.mouse_mode != mode::MouseMode::None {
            mode::write_disable_mouse(
                &mut self.canvas,
                self.modes.mouse_mode,
                self.modes.mouse_encoding,
            )
            .unwrap();
        }
        if mouse_mode != mode::MouseMode::None {
            mode::write_enable_mouse(&mut self.canvas, mouse_mode, encoding).unwrap();
        }
        self.modes.mouse_mode = mouse_mode;
        self.modes.mouse_encoding = encoding;
    }

    /// Enable/disable bracketed paste mode.
    pub fn set_bracketed_paste(&mut self, enable: bool) {
        if self.modes.bracketed_paste != enable {
            if enable {
                mode::Mode::BRACKETED_PASTE.set(&mut self.canvas).unwrap();
            } else {
                mode::Mode::BRACKETED_PASTE.reset(&mut self.canvas).unwrap();
            }
            self.modes.bracketed_paste = enable;
        }
    }

    /// Enable/disable focus in/out reporting (DECSET 1004).
    pub fn set_focus_events(&mut self, enable: bool) {
        if self.modes.focus_events != enable {
            if enable {
                mode::Mode::FOCUS.set(&mut self.canvas).unwrap();
            } else {
                mode::Mode::FOCUS.reset(&mut self.canvas).unwrap();
            }
            self.modes.focus_events = enable;
        }
    }

    /// Enable or disable color scheme update notifications
    /// (DEC private mode 2031). When enabled, the terminal sends a
    /// `CSI ? 997 ; {1|2} n` report whenever the user or operating
    /// system switches between dark and light themes; these surface as
    /// [`Event::DarkColorScheme`] / [`Event::LightColorScheme`].
    ///
    /// [`Event::DarkColorScheme`]: crate::event::Event::DarkColorScheme
    /// [`Event::LightColorScheme`]: crate::event::Event::LightColorScheme
    pub fn set_color_scheme_updates(&mut self, enable: bool) {
        if self.modes.color_scheme_updates != enable {
            if enable {
                mode::Mode::LIGHT_DARK.set(&mut self.canvas).unwrap();
            } else {
                mode::Mode::LIGHT_DARK.reset(&mut self.canvas).unwrap();
            }
            self.modes.color_scheme_updates = enable;
        }
    }

    /// Enable or disable in-band resize notifications (DEC private mode
    /// 2048). When enabled, the terminal reports every surface size
    /// change in-band as a `CSI 48 ; height ; width ; ypixel ; xpixel t`
    /// sequence, which the decoder surfaces as [`Event::Resize`] — no
    /// `SIGWINCH` handler required.
    ///
    /// [`Event::Resize`]: crate::event::Event::Resize
    pub fn set_in_band_resize(&mut self, enable: bool) {
        if self.modes.in_band_resize != enable {
            if enable {
                mode::Mode::IN_BAND_RESIZE.set(&mut self.canvas).unwrap();
            } else {
                mode::Mode::IN_BAND_RESIZE.reset(&mut self.canvas).unwrap();
            }
            self.modes.in_band_resize = enable;
        }
    }

    /// Set the window title (`OSC 0/2`).
    pub fn set_title(&mut self, title: &str) {
        ansi::write_window_title(&mut self.canvas, title).unwrap();
        self.modes.title = Some(title.to_string());
    }

    /// Set the default foreground color (`OSC 10`). Pass `Some(color)`
    /// to assign a value (converted to 24-bit RGB and emitted as
    /// `rgb:RRRR/GGGG/BBBB`); pass `None` to restore the terminal default
    /// (`OSC 110`). The choice is recorded so [`Screen::pause`] /
    /// [`Screen::finish`] can return the terminal to its built-in
    /// defaults and [`Screen::resume`] can re-apply it.
    ///
    /// [`Screen::pause`]: super::Screen::pause
    /// [`Screen::finish`]: super::Screen::finish
    /// [`Screen::resume`]: super::Screen::resume
    pub fn set_foreground_color(&mut self, color: Option<Color>) {
        if self.modes.foreground_color != color {
            match color {
                Some(c) => {
                    let (r, g, b) = c.to_rgb();
                    background::write_set_foreground_color(
                        &mut self.canvas,
                        &background::xparse_rgb(r, g, b),
                    )
                    .unwrap();
                }
                None => self
                    .canvas
                    .write_all(background::RESET_FOREGROUND_COLOR)
                    .unwrap(),
            }
            self.modes.foreground_color = color;
        }
    }

    /// Set the default background color (`OSC 11`), or restore the
    /// terminal default (`OSC 111`) when `color` is `None`. See
    /// [`Screen::set_foreground_color`](Self::set_foreground_color) for
    /// state-tracking semantics.
    pub fn set_background_color(&mut self, color: Option<Color>) {
        if self.modes.background_color != color {
            match color {
                Some(c) => {
                    let (r, g, b) = c.to_rgb();
                    background::write_set_background_color(
                        &mut self.canvas,
                        &background::xparse_rgb(r, g, b),
                    )
                    .unwrap();
                }
                None => self
                    .canvas
                    .write_all(background::RESET_BACKGROUND_COLOR)
                    .unwrap(),
            }
            self.modes.background_color = color;
        }
    }

    /// Set the cursor color (`OSC 12`), or restore the terminal default
    /// (`OSC 112`) when `color` is `None`. See
    /// [`Screen::set_foreground_color`](Self::set_foreground_color) for
    /// state-tracking semantics.
    pub fn set_cursor_color(&mut self, color: Option<Color>) {
        if self.modes.cursor_color != color {
            match color {
                Some(c) => {
                    let (r, g, b) = c.to_rgb();
                    background::write_set_cursor_color(
                        &mut self.canvas,
                        &background::xparse_rgb(r, g, b),
                    )
                    .unwrap();
                }
                None => self
                    .canvas
                    .write_all(background::RESET_CURSOR_COLOR)
                    .unwrap(),
            }
            self.modes.cursor_color = color;
        }
    }

    /// Stage the teardown of every non-render mode currently held, so the
    /// terminal is returned to a clean baseline before handing control
    /// back to the shell. Pure write — does not mutate the tracked state,
    /// so a later [`restore_modes`](Self::restore_modes) re-applies the
    /// same modes verbatim. Pairs around [`Canvas::reset`].
    pub(super) fn reset_modes(&mut self) {
        if self.modes.cursor_style != cursor::CursorStyle::Default {
            cursor::write_cursor_style(&mut self.canvas, cursor::CursorStyle::Default).unwrap();
        }
        if self.modes.bracketed_paste {
            mode::Mode::BRACKETED_PASTE.reset(&mut self.canvas).unwrap();
        }
        if self.modes.focus_events {
            mode::Mode::FOCUS.reset(&mut self.canvas).unwrap();
        }
        if self.modes.mouse_mode != mode::MouseMode::None {
            mode::write_disable_mouse(
                &mut self.canvas,
                self.modes.mouse_mode,
                self.modes.mouse_encoding,
            )
            .unwrap();
        }
        if self.modes.color_scheme_updates {
            mode::Mode::LIGHT_DARK.reset(&mut self.canvas).unwrap();
        }
        if self.modes.in_band_resize {
            mode::Mode::IN_BAND_RESIZE.reset(&mut self.canvas).unwrap();
        }
        if self.modes.foreground_color.is_some() {
            self.canvas
                .write_all(background::RESET_FOREGROUND_COLOR)
                .unwrap();
        }
        if self.modes.background_color.is_some() {
            self.canvas
                .write_all(background::RESET_BACKGROUND_COLOR)
                .unwrap();
        }
        if self.modes.cursor_color.is_some() {
            self.canvas
                .write_all(background::RESET_CURSOR_COLOR)
                .unwrap();
        }
        if self.modes.title.is_some() {
            ansi::write_window_title(&mut self.canvas, "").unwrap();
        }
    }

    /// Re-emit every non-render mode held in the tracked state. Pairs
    /// with [`reset_modes`](Self::reset_modes) for any scenario where the
    /// terminal was temporarily handed back to the shell. Pure write —
    /// does not mutate the tracked state.
    pub(super) fn restore_modes(&mut self) {
        if self.modes.cursor_style != cursor::CursorStyle::Default {
            cursor::write_cursor_style(&mut self.canvas, self.modes.cursor_style).unwrap();
        }
        if self.modes.color_scheme_updates {
            mode::Mode::LIGHT_DARK.set(&mut self.canvas).unwrap();
        }
        if self.modes.in_band_resize {
            mode::Mode::IN_BAND_RESIZE.set(&mut self.canvas).unwrap();
        }
        if self.modes.bracketed_paste {
            mode::Mode::BRACKETED_PASTE.set(&mut self.canvas).unwrap();
        }
        if self.modes.focus_events {
            mode::Mode::FOCUS.set(&mut self.canvas).unwrap();
        }
        if self.modes.mouse_mode != mode::MouseMode::None {
            mode::write_enable_mouse(
                &mut self.canvas,
                self.modes.mouse_mode,
                self.modes.mouse_encoding,
            )
            .unwrap();
        }
        if let Some(c) = self.modes.foreground_color {
            let (r, g, b) = c.to_rgb();
            background::write_set_foreground_color(
                &mut self.canvas,
                &background::xparse_rgb(r, g, b),
            )
            .unwrap();
        }
        if let Some(c) = self.modes.background_color {
            let (r, g, b) = c.to_rgb();
            background::write_set_background_color(
                &mut self.canvas,
                &background::xparse_rgb(r, g, b),
            )
            .unwrap();
        }
        if let Some(c) = self.modes.cursor_color {
            let (r, g, b) = c.to_rgb();
            background::write_set_cursor_color(&mut self.canvas, &background::xparse_rgb(r, g, b))
                .unwrap();
        }
        if let Some(title) = self.modes.title.clone() {
            ansi::write_window_title(&mut self.canvas, &title).unwrap();
        }
    }
}

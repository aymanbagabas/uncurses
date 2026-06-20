//! Non-render terminal/input mode toggles for the [`Screen`] facade —
//! cursor style, mouse tracking, bracketed paste, focus reporting,
//! color-scheme update reports, in-band resize reports, window title,
//! and the default foreground/background/cursor colors.
//!
//! Each setter emits its escape bytes through the owned [`Canvas`] and
//! flushes immediately, so the mode change takes effect on the terminal
//! right away and the call returns [`io::Result<()>`](std::io::Result). A
//! setter whose tracked value is unchanged is a no-op and performs no I/O.
//!
//! [`Screen`]: super::Screen
//! [`Canvas`]: crate::canvas::Canvas

use std::io::{self, Write};

use crate::ansi::{self, background, cursor, mode, xterm};
use crate::color::Color;
use crate::event::source::Input;

use super::Screen;
use super::cursor::CursorShape;
use super::mouse::{MouseEncoding, MouseMode};

/// DEC private modes for the mouse tracking modes and encodings this
/// library supports. Reset together to unconditionally turn mouse
/// reporting off.
const MOUSE_MODES: &[mode::Mode] = &[
    mode::Mode::MOUSE_X10,
    mode::Mode::MOUSE_NORMAL,
    mode::Mode::MOUSE_BUTTON,
    mode::Mode::MOUSE_ANY,
    mode::Mode::MOUSE_SGR,
    mode::Mode::MOUSE_SGR_PIXEL,
];

impl<I: Input, O: Write> Screen<I, O> {
    /// Set the cursor shape and blinking state (`DECSCUSR`) and flush.
    ///
    /// * `shape` — the visual cursor shape ([`Block`](CursorShape::Block),
    ///   [`Underline`](CursorShape::Underline), or [`Bar`](CursorShape::Bar)).
    /// * `blinking` — whether the cursor blinks.
    pub fn set_cursor_style(&mut self, shape: CursorShape, blinking: bool) -> io::Result<()> {
        let style = shape.style(blinking);
        cursor::write_cursor_style(&mut self.canvas, style)?;
        self.state.cursor_style = style;
        self.canvas.flush()
    }

    /// Ring the terminal bell (`BEL`) and flush.
    pub fn beep(&mut self) -> io::Result<()> {
        self.canvas.write_all(b"\x07")?;
        self.canvas.flush()
    }

    /// Set the pointer (mouse cursor) shape (`OSC 22`) and flush.
    ///
    /// `shape` is a pointer shape name such as `"default"`, `"text"`, or
    /// `"pointer"`. The shape is recorded for save/restore.
    pub fn set_pointer_shape(&mut self, shape: &str) -> io::Result<()> {
        cursor::write_set_pointer_shape(&mut self.canvas, shape)?;
        self.state.pointer_shape = Some(shape.to_string());
        self.canvas.flush()
    }

    /// Reset the pointer (mouse cursor) shape to the terminal default
    /// (`OSC 22` with an empty shape name) and flush.
    pub fn reset_pointer_shape(&mut self) -> io::Result<()> {
        cursor::write_set_pointer_shape(&mut self.canvas, "")?;
        self.state.pointer_shape = None;
        self.canvas.flush()
    }

    /// Enable mouse tracking and flush, choosing the best mode and encoding
    /// the terminal is known to support (see
    /// [`capabilities`](Self::capabilities)).
    ///
    /// * `motion` — report pointer motion. Uses any-event tracking when
    ///   supported, else button-event, else plain button tracking.
    /// * `pixels` — request coordinates in pixels rather than cells. Not all
    ///   terminals support pixel reporting, so this first checks the
    ///   detected [`mouse_sgr_pixel`](crate::screen::Capabilities::mouse_sgr_pixel)
    ///   capability: SGR-pixel encoding is used only when supported,
    ///   otherwise it falls back to SGR (cell) encoding, then the legacy
    ///   encoding. When pixel reporting is active, a
    ///   [`Mouse`](crate::event::Mouse) event's pixel coordinates can be
    ///   converted to cells with
    ///   [`mouse_pixel_to_cell`](crate::event::mouse_pixel_to_cell) using
    ///   the terminal's pixel size (from
    ///   [`request_window_pixel_size`](Self::request_window_pixel_size) →
    ///   [`Event::WindowPixelSize`](crate::event::Event::WindowPixelSize))
    ///   and its cell size ([`size`](Self::size)).
    ///
    /// The chosen mode and encoding are recorded for save/restore.
    pub fn enable_mouse(&mut self, motion: bool, pixels: bool) -> io::Result<()> {
        let encoding = if pixels && self.caps.mouse_sgr_pixel {
            MouseEncoding::SgrPixel
        } else if self.caps.mouse_sgr {
            MouseEncoding::Sgr
        } else {
            MouseEncoding::X10
        };
        let mouse_mode = if motion && self.caps.mouse_any {
            MouseMode::Any
        } else if self.caps.mouse_button {
            MouseMode::Button
        } else {
            MouseMode::Normal
        };
        // Drop any prior tracking first so encodings/modes don't stack.
        mode::write_reset_mode(&mut self.canvas, MOUSE_MODES)?;
        self.write_mouse_modes(mouse_mode, encoding, true)?;
        self.state.mouse_mode = Some(mouse_mode);
        self.state.mouse_encoding = encoding;
        self.canvas.flush()
    }

    /// Disable all mouse tracking modes and encodings, and flush.
    pub fn disable_mouse(&mut self) -> io::Result<()> {
        mode::write_reset_mode(&mut self.canvas, MOUSE_MODES)?;
        self.state.mouse_mode = None;
        self.canvas.flush()
    }

    /// Set or reset the tracking mode and encoding for the given mouse
    /// configuration. `enable` selects set vs reset.
    fn write_mouse_modes(
        &mut self,
        mouse_mode: MouseMode,
        encoding: MouseEncoding,
        enable: bool,
    ) -> io::Result<()> {
        let mut modes = vec![mouse_mode.dec_mode()];
        if let Some(e) = encoding.dec_mode() {
            modes.push(e);
        }
        if enable {
            mode::write_set_mode(&mut self.canvas, &modes)
        } else {
            mode::write_reset_mode(&mut self.canvas, &modes)
        }
    }

    /// Enable bracketed paste mode (DEC private mode 2004) and flush.
    pub fn enable_bracketed_paste(&mut self) -> io::Result<()> {
        mode::Mode::BRACKETED_PASTE.set(&mut self.canvas)?;
        self.state.bracketed_paste = true;
        self.canvas.flush()
    }

    /// Disable bracketed paste mode (DEC private mode 2004) and flush.
    pub fn disable_bracketed_paste(&mut self) -> io::Result<()> {
        mode::Mode::BRACKETED_PASTE.reset(&mut self.canvas)?;
        self.state.bracketed_paste = false;
        self.canvas.flush()
    }

    /// Enable focus in/out reporting (DEC private mode 1004) and flush.
    pub fn enable_focus_events(&mut self) -> io::Result<()> {
        mode::Mode::FOCUS.set(&mut self.canvas)?;
        self.state.focus_events = true;
        self.canvas.flush()
    }

    /// Disable focus in/out reporting (DEC private mode 1004) and flush.
    pub fn disable_focus_events(&mut self) -> io::Result<()> {
        mode::Mode::FOCUS.reset(&mut self.canvas)?;
        self.state.focus_events = false;
        self.canvas.flush()
    }

    /// Enable color-theme update notifications (DEC private mode 2031) and
    /// flush. The terminal then sends a `CSI ? 997 ; {1|2} n` report
    /// whenever the user or operating system switches between dark and
    /// light themes; these surface as [`Event::ColorTheme`]. The report
    /// indicates only the dark/light preference, not the actual colors.
    ///
    /// [`Event::ColorTheme`]: crate::event::Event::ColorTheme
    pub fn enable_color_theme_updates(&mut self) -> io::Result<()> {
        mode::Mode::LIGHT_DARK.set(&mut self.canvas)?;
        self.state.color_theme_updates = true;
        self.canvas.flush()
    }

    /// Disable color-theme update notifications (DEC private mode 2031) and
    /// flush.
    pub fn disable_color_theme_updates(&mut self) -> io::Result<()> {
        mode::Mode::LIGHT_DARK.reset(&mut self.canvas)?;
        self.state.color_theme_updates = false;
        self.canvas.flush()
    }

    /// Enable in-band resize notifications (DEC private mode 2048) and
    /// flush. The terminal then reports every surface size change in-band
    /// as a `CSI 48 ; height ; width ; ypixel ; xpixel t` sequence, which
    /// the decoder surfaces as [`Event::Resize`] — no `SIGWINCH` handler
    /// required.
    ///
    /// [`Event::Resize`]: crate::event::Event::Resize
    pub fn enable_in_band_resize(&mut self) -> io::Result<()> {
        mode::Mode::IN_BAND_RESIZE.set(&mut self.canvas)?;
        self.state.in_band_resize = true;
        self.canvas.flush()
    }

    /// Disable in-band resize notifications (DEC private mode 2048) and
    /// flush.
    pub fn disable_in_band_resize(&mut self) -> io::Result<()> {
        mode::Mode::IN_BAND_RESIZE.reset(&mut self.canvas)?;
        self.state.in_band_resize = false;
        self.canvas.flush()
    }

    /// Set the window title (`OSC 2`) and flush.
    pub fn set_title(&mut self, title: &str) -> io::Result<()> {
        ansi::write_window_title(&mut self.canvas, title)?;
        self.state.title = Some(title.to_string());
        self.canvas.flush()
    }

    /// Set the xterm modifyOtherKeys mode (`CSI > 4 ; n m`) and flush.
    /// Passing [`ModifyOtherKeysMode::Disabled`] resets it (`CSI > 4 m`).
    /// The mode is recorded so [`Screen::finish`](super::Screen::finish)
    /// can reset it and [`Screen::resume`](super::Screen::resume) re-apply
    /// it.
    ///
    /// [`ModifyOtherKeysMode::Disabled`]: crate::event::ModifyOtherKeysMode::Disabled
    pub fn set_modify_other_keys(
        &mut self,
        mode: crate::event::ModifyOtherKeysMode,
    ) -> io::Result<()> {
        use crate::event::ModifyOtherKeysMode;
        match mode {
            ModifyOtherKeysMode::Disabled => {
                self.canvas.write_all(xterm::RESET_MODIFY_OTHER_KEYS)?
            }
            ModifyOtherKeysMode::Mode1 => self.canvas.write_all(xterm::SET_MODIFY_OTHER_KEYS_1)?,
            ModifyOtherKeysMode::Mode2 => self.canvas.write_all(xterm::SET_MODIFY_OTHER_KEYS_2)?,
        }
        self.state.modify_other_keys = mode;
        self.canvas.flush()
    }

    /// Set the default foreground color (`OSC 10`) and flush. The color is
    /// converted to 24-bit RGB and emitted as `rgb:RRRR/GGGG/BBBB`, and is
    /// recorded so [`Screen::finish`](super::Screen::finish) can restore
    /// the terminal default and [`Screen::resume`](super::Screen::resume)
    /// can re-apply it.
    pub fn set_foreground_color(&mut self, color: Color) -> io::Result<()> {
        let (r, g, b) = color.to_rgb();
        background::write_set_foreground_color(&mut self.canvas, &background::xparse_rgb(r, g, b))?;
        self.state.foreground_color = Some(color);
        self.canvas.flush()
    }

    /// Restore the terminal's default foreground color (`OSC 110`) and
    /// flush.
    pub fn reset_foreground_color(&mut self) -> io::Result<()> {
        self.canvas.write_all(background::RESET_FOREGROUND_COLOR)?;
        self.state.foreground_color = None;
        self.canvas.flush()
    }

    /// Set the default background color (`OSC 11`) and flush. See
    /// [`set_foreground_color`](Self::set_foreground_color) for
    /// state-tracking semantics.
    pub fn set_background_color(&mut self, color: Color) -> io::Result<()> {
        let (r, g, b) = color.to_rgb();
        background::write_set_background_color(&mut self.canvas, &background::xparse_rgb(r, g, b))?;
        self.state.background_color = Some(color);
        self.canvas.flush()
    }

    /// Restore the terminal's default background color (`OSC 111`) and
    /// flush.
    pub fn reset_background_color(&mut self) -> io::Result<()> {
        self.canvas.write_all(background::RESET_BACKGROUND_COLOR)?;
        self.state.background_color = None;
        self.canvas.flush()
    }

    /// Set the cursor color (`OSC 12`) and flush. See
    /// [`set_foreground_color`](Self::set_foreground_color) for
    /// state-tracking semantics.
    pub fn set_cursor_color(&mut self, color: Color) -> io::Result<()> {
        let (r, g, b) = color.to_rgb();
        background::write_set_cursor_color(&mut self.canvas, &background::xparse_rgb(r, g, b))?;
        self.state.cursor_color = Some(color);
        self.canvas.flush()
    }

    /// Restore the terminal's default cursor color (`OSC 112`) and flush.
    pub fn reset_cursor_color(&mut self) -> io::Result<()> {
        self.canvas.write_all(background::RESET_CURSOR_COLOR)?;
        self.state.cursor_color = None;
        self.canvas.flush()
    }

    /// Set a terminal palette color by index (`OSC 4`) and flush. The
    /// override is tracked so [`Screen::finish`](super::Screen::finish) can
    /// restore it and [`Screen::resume`](super::Screen::resume) re-apply it.
    pub fn set_palette_color(&mut self, index: u8, color: Color) -> io::Result<()> {
        let (r, g, b) = color.to_rgb();
        background::write_set_palette_color(
            &mut self.canvas,
            index,
            &background::xparse_rgb(r, g, b),
        )?;
        self.state.palette.insert(index, color);
        self.canvas.flush()
    }

    /// Reset a single terminal palette color to its default
    /// (`OSC 104 ; index`) and flush.
    pub fn reset_palette_color(&mut self, index: u8) -> io::Result<()> {
        background::write_reset_palette_color(&mut self.canvas, index)?;
        self.state.palette.remove(&index);
        self.canvas.flush()
    }

    /// Reset the entire terminal palette to its defaults (`OSC 104`) and
    /// flush, clearing every tracked palette override.
    pub fn reset_palette_colors(&mut self) -> io::Result<()> {
        self.canvas.write_all(background::RESET_PALETTE_COLORS)?;
        self.state.palette.clear();
        self.canvas.flush()
    }

    /// Stage the teardown of every non-render mode currently held, so the
    /// terminal is returned to a clean baseline before handing control
    /// back to the shell. Pure write — does not mutate the tracked state,
    /// so a later [`restore_modes`](Self::restore_modes) re-applies the
    /// same modes verbatim. Pairs around [`Canvas::reset`]. The caller
    /// flushes.
    pub(super) fn reset_modes(&mut self) -> io::Result<()> {
        if self.state.cursor_style != cursor::CursorStyle::Default {
            cursor::write_cursor_style(&mut self.canvas, cursor::CursorStyle::Default)?;
        }
        if self.state.bracketed_paste {
            mode::Mode::BRACKETED_PASTE.reset(&mut self.canvas)?;
        }
        if self.state.focus_events {
            mode::Mode::FOCUS.reset(&mut self.canvas)?;
        }
        if let Some(m) = self.state.mouse_mode {
            self.write_mouse_modes(m, self.state.mouse_encoding, false)?;
        }
        if self.state.color_theme_updates {
            mode::Mode::LIGHT_DARK.reset(&mut self.canvas)?;
        }
        if self.state.in_band_resize {
            mode::Mode::IN_BAND_RESIZE.reset(&mut self.canvas)?;
        }
        if self.state.modify_other_keys != crate::event::ModifyOtherKeysMode::Disabled {
            self.canvas.write_all(xterm::RESET_MODIFY_OTHER_KEYS)?;
        }
        if self.state.foreground_color.is_some() {
            self.canvas.write_all(background::RESET_FOREGROUND_COLOR)?;
        }
        if self.state.background_color.is_some() {
            self.canvas.write_all(background::RESET_BACKGROUND_COLOR)?;
        }
        if self.state.cursor_color.is_some() {
            self.canvas.write_all(background::RESET_CURSOR_COLOR)?;
        }
        for &index in self.state.palette.keys() {
            background::write_reset_palette_color(&mut self.canvas, index)?;
        }
        if self.state.title.is_some() {
            ansi::write_window_title(&mut self.canvas, "")?;
        }
        if self.state.pointer_shape.is_some() {
            cursor::write_set_pointer_shape(&mut self.canvas, "")?;
        }
        Ok(())
    }

    /// Re-emit every non-render mode held in the tracked state. Pairs
    /// with [`reset_modes`](Self::reset_modes) for any scenario where the
    /// terminal was temporarily handed back to the shell. Pure write —
    /// does not mutate the tracked state. The caller flushes.
    pub(super) fn restore_modes(&mut self) -> io::Result<()> {
        if self.state.cursor_style != cursor::CursorStyle::Default {
            cursor::write_cursor_style(&mut self.canvas, self.state.cursor_style)?;
        }
        if self.state.color_theme_updates {
            mode::Mode::LIGHT_DARK.set(&mut self.canvas)?;
        }
        if self.state.in_band_resize {
            mode::Mode::IN_BAND_RESIZE.set(&mut self.canvas)?;
        }
        match self.state.modify_other_keys {
            crate::event::ModifyOtherKeysMode::Mode1 => {
                self.canvas.write_all(xterm::SET_MODIFY_OTHER_KEYS_1)?;
            }
            crate::event::ModifyOtherKeysMode::Mode2 => {
                self.canvas.write_all(xterm::SET_MODIFY_OTHER_KEYS_2)?;
            }
            crate::event::ModifyOtherKeysMode::Disabled => {}
        }
        if self.state.bracketed_paste {
            mode::Mode::BRACKETED_PASTE.set(&mut self.canvas)?;
        }
        if self.state.focus_events {
            mode::Mode::FOCUS.set(&mut self.canvas)?;
        }
        if let Some(m) = self.state.mouse_mode {
            self.write_mouse_modes(m, self.state.mouse_encoding, true)?;
        }
        if let Some(c) = self.state.foreground_color {
            let (r, g, b) = c.to_rgb();
            background::write_set_foreground_color(
                &mut self.canvas,
                &background::xparse_rgb(r, g, b),
            )?;
        }
        if let Some(c) = self.state.background_color {
            let (r, g, b) = c.to_rgb();
            background::write_set_background_color(
                &mut self.canvas,
                &background::xparse_rgb(r, g, b),
            )?;
        }
        if let Some(c) = self.state.cursor_color {
            let (r, g, b) = c.to_rgb();
            background::write_set_cursor_color(&mut self.canvas, &background::xparse_rgb(r, g, b))?;
        }
        for (&index, &c) in &self.state.palette {
            let (r, g, b) = c.to_rgb();
            background::write_set_palette_color(
                &mut self.canvas,
                index,
                &background::xparse_rgb(r, g, b),
            )?;
        }
        if let Some(title) = self.state.title.clone() {
            ansi::write_window_title(&mut self.canvas, &title)?;
        }
        if let Some(shape) = self.state.pointer_shape.clone() {
            cursor::write_set_pointer_shape(&mut self.canvas, &shape)?;
        }
        Ok(())
    }

    // --- Request delegates -----------------------------------------------
    //
    // Each writes a terminal query and flushes; the reply arrives later
    // through the event flow. Replies that double as init capability
    // reports (mode, kitty keyboard) are recorded into
    // [`capabilities`](Self::capabilities); value replies (cursor
    // position, colors, pixel sizes) surface to the caller as events.

    /// Request the window size in pixels (XTWINOPS `CSI 14 t`). Reply:
    /// [`Event::WindowPixelSize`](crate::event::Event::WindowPixelSize).
    pub fn request_window_pixel_size(&mut self) -> io::Result<()> {
        self.canvas
            .write_all(crate::ansi::winop::REQUEST_WINDOW_PIXEL_SIZE)?;
        self.canvas.flush()
    }

    /// Request the character cell size in pixels (XTWINOPS `CSI 16 t`).
    /// Reply: [`Event::CellPixelSize`](crate::event::Event::CellPixelSize).
    pub fn request_cell_pixel_size(&mut self) -> io::Result<()> {
        self.canvas
            .write_all(crate::ansi::winop::REQUEST_CELL_PIXEL_SIZE)?;
        self.canvas.flush()
    }

    /// Request the terminal's active Kitty keyboard flags (`CSI ? u`).
    /// The reply is recorded in [`capabilities`](Self::capabilities).
    pub fn request_kitty_keyboard(&mut self) -> io::Result<()> {
        self.canvas
            .write_all(crate::ansi::kitty::REQUEST_KITTY_KEYBOARD)?;
        self.canvas.flush()
    }

    /// Request the terminal's modifyOtherKeys state (`CSI ? 4 m`). The
    /// reply is recorded in [`capabilities`](Self::capabilities).
    pub fn request_modify_other_keys(&mut self) -> io::Result<()> {
        self.canvas
            .write_all(crate::ansi::xterm::QUERY_MODIFY_OTHER_KEYS)?;
        self.canvas.flush()
    }

    /// Request the default foreground color (`OSC 10 ; ? ST`). Reply:
    /// [`Event::ForegroundColor`](crate::event::Event::ForegroundColor).
    pub fn request_foreground_color(&mut self) -> io::Result<()> {
        self.canvas
            .write_all(crate::ansi::background::REQUEST_FOREGROUND_COLOR)?;
        self.canvas.flush()
    }

    /// Request the default background color (`OSC 11 ; ? ST`). Reply:
    /// [`Event::BackgroundColor`](crate::event::Event::BackgroundColor).
    pub fn request_background_color(&mut self) -> io::Result<()> {
        self.canvas
            .write_all(crate::ansi::background::REQUEST_BACKGROUND_COLOR)?;
        self.canvas.flush()
    }

    /// Request the cursor color (`OSC 12 ; ? ST`). Reply:
    /// [`Event::CursorColor`](crate::event::Event::CursorColor).
    pub fn request_cursor_color(&mut self) -> io::Result<()> {
        self.canvas
            .write_all(crate::ansi::background::REQUEST_CURSOR_COLOR)?;
        self.canvas.flush()
    }

    /// Request a terminal palette color by index (`OSC 4 ; index ; ? ST`).
    /// Reply: `OSC 4 ; index ; rgb:... ST`.
    pub fn request_palette_color(&mut self, index: u8) -> io::Result<()> {
        crate::ansi::background::write_request_palette_color(&mut self.canvas, index)?;
        self.canvas.flush()
    }

    /// Request a terminal mode's current setting (DECRQM). Reply:
    /// [`Event::ModeReport`](crate::event::Event::ModeReport).
    pub fn request_mode(&mut self, mode: crate::ansi::mode::Mode) -> io::Result<()> {
        mode.request(&mut self.canvas)?;
        self.canvas.flush()
    }

    /// Request the cursor position (`CSI 6 n`). Reply:
    /// [`Event::CursorPosition`](crate::event::Event::CursorPosition).
    pub fn request_cursor_position(&mut self) -> io::Result<()> {
        self.canvas
            .write_all(crate::ansi::status::REQUEST_CURSOR_POSITION)?;
        self.canvas.flush()
    }

    /// Request the current color theme (`CSI ? 996 n`): whether the
    /// terminal's theme is dark or light. This reports only the dark/light
    /// preference, not the actual colors. Reply:
    /// [`Event::ColorTheme`](crate::event::Event::ColorTheme).
    pub fn request_color_theme(&mut self) -> io::Result<()> {
        self.canvas
            .write_all(crate::ansi::status::REQUEST_LIGHT_DARK_REPORT)?;
        self.canvas.flush()
    }

    /// Set the system clipboard contents (`OSC 52 ; c`). `data` is
    /// base64-encoded for transport.
    pub fn set_system_clipboard(&mut self, data: &[u8]) -> io::Result<()> {
        crate::ansi::clipboard::write_set_clipboard(
            &mut self.canvas,
            crate::ansi::clipboard::SYSTEM_CLIPBOARD,
            data,
        )?;
        self.canvas.flush()
    }

    /// Set the primary selection contents (`OSC 52 ; p`). `data` is
    /// base64-encoded for transport.
    pub fn set_primary_clipboard(&mut self, data: &[u8]) -> io::Result<()> {
        crate::ansi::clipboard::write_set_clipboard(
            &mut self.canvas,
            crate::ansi::clipboard::PRIMARY_CLIPBOARD,
            data,
        )?;
        self.canvas.flush()
    }

    /// Request the system clipboard contents (`OSC 52 ; c ; ?`). Reply:
    /// [`Event::Clipboard`](crate::event::Event::Clipboard).
    pub fn request_system_clipboard(&mut self) -> io::Result<()> {
        crate::ansi::clipboard::write_request_clipboard(
            &mut self.canvas,
            crate::ansi::clipboard::SYSTEM_CLIPBOARD,
        )?;
        self.canvas.flush()
    }

    /// Request the primary selection contents (`OSC 52 ; p ; ?`). Reply:
    /// [`Event::Clipboard`](crate::event::Event::Clipboard).
    pub fn request_primary_clipboard(&mut self) -> io::Result<()> {
        crate::ansi::clipboard::write_request_clipboard(
            &mut self.canvas,
            crate::ansi::clipboard::PRIMARY_CLIPBOARD,
        )?;
        self.canvas.flush()
    }
}

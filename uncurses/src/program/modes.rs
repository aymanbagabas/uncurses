//! Non-render terminal/input mode toggles for the [`Program`] facade —
//! cursor style, mouse tracking, bracketed paste, focus reporting,
//! color-scheme update reports, in-band resize reports, window title,
//! progress reports, and the default foreground/background/cursor colors.
//!
//! Each setter emits its escape bytes through the owned renderer and
//! flushes immediately, so the mode change takes effect on the terminal
//! right away and the call returns [`io::Result<()>`](std::io::Result). A
//! setter whose tracked value is unchanged is a no-op and performs no I/O.
//!
//! [`Program`]: super::Program

use std::io::{self, Write};

use crate::ansi::{self, color, cursor, kitty, mode, progress, xterm};
use crate::color::Color;
use crate::event::Input;

use super::MouseTracking;
use super::Program;
use super::ProgressState;
use super::cursor::CursorShape;

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

impl<I: Input, O: Write> Program<I, O> {
    /// Set the cursor shape and blinking state (`DECSCUSR`) and flush.
    ///
    /// * `shape` — the visual cursor shape ([`Block`](CursorShape::Block),
    ///   [`Underline`](CursorShape::Underline), or [`Bar`](CursorShape::Bar)).
    /// * `blinking` — whether the cursor blinks.
    pub fn set_cursor_style(&mut self, shape: CursorShape, blinking: bool) -> io::Result<()> {
        let style = shape.style(blinking);
        cursor::write_cursor_style(&mut self.screen, style)?;
        self.state.cursor_style = style;
        self.screen.flush()
    }

    /// Ring the terminal bell (`BEL`) and flush.
    pub fn beep(&mut self) -> io::Result<()> {
        self.screen.write_all(b"\x07")?;
        self.screen.flush()
    }

    /// Set the pointer (mouse cursor) shape (`OSC 22`) and flush.
    ///
    /// `shape` is a pointer shape name such as `"default"`, `"text"`, or
    /// `"pointer"`. The shape is recorded for save/restore.
    pub fn set_pointer_shape(&mut self, shape: &str) -> io::Result<()> {
        cursor::write_set_pointer_shape(&mut self.screen, shape)?;
        self.state.pointer_shape = Some(shape.to_string());
        self.screen.flush()
    }

    /// Reset the pointer (mouse cursor) shape to the terminal default
    /// (`OSC 22 ; default`) and flush.
    ///
    /// Uses the explicit `"default"` shape name rather than an empty one: some
    /// terminals don't treat an empty `OSC 22` as a reset.
    pub fn reset_pointer_shape(&mut self) -> io::Result<()> {
        cursor::write_set_pointer_shape(&mut self.screen, "default")?;
        self.state.pointer_shape = None;
        self.screen.flush()
    }

    /// Report progress to the terminal (`OSC 9;4`) and flush.
    ///
    /// Terminals that support it show the progress in the taskbar, tab, or
    /// window chrome; the rest ignore the sequence. Percentages are clamped
    /// to `0..=100`.
    ///
    /// The state is recorded for save/restore: it is removed on a shell
    /// handoff ([`pause`](Self::pause), [`suspend`](Self::suspend),
    /// [`finish`](Self::finish)) and re-reported by
    /// [`resume`](Self::resume). Take it down with
    /// [`reset_progress_state`](Self::reset_progress_state).
    pub fn set_progress_state(&mut self, progress: ProgressState) -> io::Result<()> {
        progress.write(&mut self.screen)?;
        self.state.progress = Some(progress);
        self.screen.flush()
    }

    /// Remove the progress report (`OSC 9;4;0`) and flush.
    pub fn reset_progress_state(&mut self) -> io::Result<()> {
        self.screen.write_all(progress::RESET_PROGRESS_BAR)?;
        self.state.progress = None;
        self.screen.flush()
    }

    /// Enable mouse tracking and flush.
    ///
    /// This emits exactly what is asked for and does not consult terminal
    /// [`capabilities`](Self::capabilities). Unsupported modes are ignored by
    /// the terminal, and because the mode requests are mutually exclusive, each
    /// terminal settles on the most capable variant it understands:
    ///
    /// * Tracking: button (`1000`) and button-event (`1002`) are always
    ///   requested, so a terminal reports drag where it can and plain clicks
    ///   otherwise. With [`MouseTracking::MOTION`], any-event tracking (`1003`)
    ///   is added on top, so motion without a button held is reported where
    ///   supported.
    /// * Encoding: SGR (`1006`) is always requested, since the legacy byte
    ///   encoding caps coordinates at 223 and SGR is universally supported.
    ///   With [`MouseTracking::PIXELS`], SGR-pixel (`1016`) is added; terminals
    ///   that support it report pixel coordinates, and the rest fall back to
    ///   SGR cell coordinates.
    ///
    /// Pass [`MouseTracking::empty()`] for basic button tracking with no
    /// extras. To turn mouse tracking off, call [`disable_mouse`](Self::disable_mouse).
    ///
    /// To learn which variant a terminal actually chose, read
    /// [`capabilities`](Self::capabilities) (for example
    /// [`supports(Mode::MOUSE_SGR_PIXEL)`](super::Capabilities::supports) to
    /// tell whether pixels or cells will arrive). When pixel reporting is active, a
    /// [`Mouse`](crate::event::Mouse) event's pixel coordinates can be converted
    /// to cells with [`mouse_pixels_to_cells`](Self::mouse_pixels_to_cells).
    ///
    /// Mouse coordinates are physical screen coordinates. Inline, follow this
    /// with [`request_origin`](Self::request_origin) to learn where the managed
    /// area sits, so [`mouse_to_origin`](Self::mouse_to_origin) can map them
    /// into it.
    ///
    /// The request is recorded for save/restore.
    pub fn enable_mouse(&mut self, tracking: MouseTracking) -> io::Result<()> {
        // Drop any prior tracking first so modes don't stack ambiguously.
        mode::write_reset_mode(&mut self.screen, MOUSE_MODES)?;
        self.write_mouse_modes(tracking, true)?;
        self.state.mouse = Some(tracking);
        self.screen.flush()
    }

    /// Disable all mouse tracking modes and encodings, and flush.
    pub fn disable_mouse(&mut self) -> io::Result<()> {
        mode::write_reset_mode(&mut self.screen, MOUSE_MODES)?;
        self.state.mouse = None;
        self.screen.flush()
    }

    /// Set or reset the mouse tracking modes and encoding for the given
    /// tracking flags. `enable` selects set vs reset. The modes are emitted in
    /// ascending order so that, where the requests are mutually exclusive, the
    /// most capable supported variant wins (`1003` over `1002`/`1000`, `1016`
    /// over `1006`).
    fn write_mouse_modes(&mut self, tracking: MouseTracking, enable: bool) -> io::Result<()> {
        // Always request plain and button-event tracking as a fallback pair,
        // adding any-event tracking on top when motion is requested.
        let mut modes = vec![mode::Mode::MOUSE_NORMAL, mode::Mode::MOUSE_BUTTON];
        if tracking.contains(MouseTracking::MOTION) {
            modes.push(mode::Mode::MOUSE_ANY);
        }
        // Always request SGR encoding; add SGR-pixel on top when pixels are
        // requested. Terminals without pixel support fall back to SGR cells.
        modes.push(mode::Mode::MOUSE_SGR);
        if tracking.contains(MouseTracking::PIXELS) {
            modes.push(mode::Mode::MOUSE_SGR_PIXEL);
        }
        if enable {
            mode::write_set_mode(&mut self.screen, &modes)
        } else {
            mode::write_reset_mode(&mut self.screen, &modes)
        }
    }

    /// Enable bracketed paste mode (DEC private mode 2004) and flush.
    pub fn enable_bracketed_paste(&mut self) -> io::Result<()> {
        mode::Mode::BRACKETED_PASTE.set(&mut self.screen)?;
        self.state.bracketed_paste = true;
        self.screen.flush()
    }

    /// Disable bracketed paste mode (DEC private mode 2004) and flush.
    pub fn disable_bracketed_paste(&mut self) -> io::Result<()> {
        mode::Mode::BRACKETED_PASTE.reset(&mut self.screen)?;
        self.state.bracketed_paste = false;
        self.screen.flush()
    }

    /// Enable focus in/out reporting (DEC private mode 1004) and flush.
    pub fn enable_focus_events(&mut self) -> io::Result<()> {
        mode::Mode::FOCUS.set(&mut self.screen)?;
        self.state.focus_events = true;
        self.screen.flush()
    }

    /// Disable focus in/out reporting (DEC private mode 1004) and flush.
    pub fn disable_focus_events(&mut self) -> io::Result<()> {
        mode::Mode::FOCUS.reset(&mut self.screen)?;
        self.state.focus_events = false;
        self.screen.flush()
    }

    /// Enable color-scheme update notifications (DEC private mode 2031) and
    /// flush. The terminal then sends a `CSI ? 997 ; {1|2} n` report
    /// whenever the user or operating system switches between dark and
    /// light schemes; these surface as [`Event::ColorScheme`]. The report
    /// indicates only the dark/light preference, not the actual colors.
    ///
    /// [`Event::ColorScheme`]: crate::event::Event::ColorScheme
    pub fn enable_color_scheme_updates(&mut self) -> io::Result<()> {
        mode::Mode::LIGHT_DARK.set(&mut self.screen)?;
        self.state.color_scheme_updates = true;
        self.screen.flush()
    }

    /// Disable color-scheme update notifications (DEC private mode 2031) and
    /// flush.
    pub fn disable_color_scheme_updates(&mut self) -> io::Result<()> {
        mode::Mode::LIGHT_DARK.reset(&mut self.screen)?;
        self.state.color_scheme_updates = false;
        self.screen.flush()
    }

    /// Enable in-band resize notifications (DEC private mode 2048) and
    /// flush. The terminal then reports every surface size change in-band
    /// as a `CSI 48 ; height ; width ; ypixel ; xpixel t` sequence, which
    /// the decoder surfaces as [`Event::Resize`] — no `SIGWINCH` handler
    /// required. The event source stops synthesizing resizes from `SIGWINCH`
    /// while this is on, so a size change is reported once, not twice.
    ///
    /// Only call this after [`capabilities`](Self::capabilities) reports
    /// [`supports(Mode::IN_BAND_RESIZE)`](super::Capabilities::supports); a
    /// terminal that ignores the mode would otherwise leave you with no
    /// resize events at all.
    ///
    /// [`Event::Resize`]: crate::event::Event::Resize
    pub fn enable_in_band_resize(&mut self) -> io::Result<()> {
        mode::Mode::IN_BAND_RESIZE.set(&mut self.screen)?;
        self.state.in_band_resize = true;
        self.source.lock().unwrap().set_handle_resize(false);
        self.screen.flush()
    }

    /// Disable in-band resize notifications (DEC private mode 2048) and
    /// flush, handing resize reporting back to the `SIGWINCH` path.
    pub fn disable_in_band_resize(&mut self) -> io::Result<()> {
        mode::Mode::IN_BAND_RESIZE.reset(&mut self.screen)?;
        self.state.in_band_resize = false;
        self.source.lock().unwrap().set_handle_resize(true);
        self.screen.flush()
    }

    /// Set both the window title and icon name (`OSC 0`) and flush.
    ///
    /// An empty `title` clears both overrides, restoring the terminal's
    /// defaults; the state is recorded as unset so teardown and resume skip
    /// them. To set just one, use
    /// [`set_window_title`](Self::set_window_title) (`OSC 2`) or
    /// [`set_icon_title`](Self::set_icon_title) (`OSC 1`).
    pub fn set_title(&mut self, title: &str) -> io::Result<()> {
        ansi::title::write_window_title_and_icon(&mut self.screen, title)?;
        let stored = (!title.is_empty()).then(|| title.to_string());
        self.state.window_title = stored.clone();
        self.state.icon_name = stored;
        self.screen.flush()
    }

    /// Set the window title only (`OSC 2`) and flush.
    ///
    /// An empty `title` clears the override, restoring the terminal's default
    /// window title. Unlike [`set_title`](Self::set_title) (`OSC 0`), this
    /// leaves the icon name untouched.
    pub fn set_window_title(&mut self, title: &str) -> io::Result<()> {
        ansi::title::write_window_title(&mut self.screen, title)?;
        self.state.window_title = (!title.is_empty()).then(|| title.to_string());
        self.screen.flush()
    }

    /// Set the icon name only (`OSC 1`) and flush.
    ///
    /// An empty `title` clears the override, restoring the terminal's default
    /// icon name. Unlike [`set_title`](Self::set_title) (`OSC 0`), this leaves
    /// the window title untouched.
    pub fn set_icon_title(&mut self, title: &str) -> io::Result<()> {
        ansi::title::write_icon_name(&mut self.screen, title)?;
        self.state.icon_name = (!title.is_empty()).then(|| title.to_string());
        self.screen.flush()
    }

    /// Enter the alternate screen buffer (DECSET 1049) and flush.
    ///
    /// The managed area becomes the whole viewport: the screen switches to
    /// absolute addressing and repaints in full on the next
    /// [`render`](crate::screen::Screen::render). The normal buffer, its scrollback, and the
    /// shell prompt are left untouched underneath and come back on
    /// [`exit_alt_screen`](Self::exit_alt_screen).
    ///
    /// Cursor visibility and the Kitty keyboard stack are per-screen-buffer
    /// on some terminals, so both are re-asserted on the newly active buffer.
    pub fn enter_alt_screen(&mut self) -> io::Result<()> {
        self.set_alt_screen(true)
    }

    /// Leave the alternate screen buffer (DECRST 1049) and flush, restoring
    /// the normal buffer and its scrollback. The managed area becomes an
    /// inline band again, addressed with relative moves.
    pub fn exit_alt_screen(&mut self) -> io::Result<()> {
        self.set_alt_screen(false)
    }

    /// Emit the alternate-screen switch and bring the screen and the
    /// per-buffer modes with it. Always emits the mode; the bookkeeping runs
    /// only on an actual transition.
    pub(super) fn set_alt_screen(&mut self, enter: bool) -> io::Result<()> {
        let changed = self.state.alt_screen != enter;
        if changed && enter {
            // Capture the inline anchor before the buffer switch hides it.
            self.screen.save_cursor();
        }
        if enter {
            mode::Mode::ALT_SCREEN_SAVE_CURSOR.set(&mut self.screen)?;
        } else {
            mode::Mode::ALT_SCREEN_SAVE_CURSOR.reset(&mut self.screen)?;
        }
        self.state.alt_screen = enter;
        if changed {
            self.screen.set_fullscreen(enter);
            // Some terminals track DECTCEM and the Kitty keyboard stack per
            // screen buffer, so the newly active one can disagree with what
            // this program asked for. Re-assert both.
            if !self.state.cursor_visible {
                mode::Mode::CURSOR_VISIBLE.reset(&mut self.screen)?;
            } else {
                mode::Mode::CURSOR_VISIBLE.set(&mut self.screen)?;
            }
            if !self.state.kitty_keyboard.is_empty() {
                kitty::write_set_kitty_keyboard(
                    &mut self.screen,
                    self.state.kitty_keyboard,
                    kitty::KittyKeyboardMode::Set,
                )?;
            }
        }
        self.screen.flush()
    }

    /// Show the terminal cursor (DECSET 25) and flush.
    ///
    /// Also tells the screen, which hides a visible cursor around each frame's
    /// cell diff so it does not dance across cells as the renderer
    /// repositions it.
    pub fn show_cursor(&mut self) -> io::Result<()> {
        mode::Mode::CURSOR_VISIBLE.set(&mut self.screen)?;
        self.state.cursor_visible = true;
        self.screen.set_cursor_visible(true);
        self.screen.flush()
    }

    /// Hide the terminal cursor (DECRST 25) and flush.
    pub fn hide_cursor(&mut self) -> io::Result<()> {
        mode::Mode::CURSOR_VISIBLE.reset(&mut self.screen)?;
        self.state.cursor_visible = false;
        self.screen.set_cursor_visible(false);
        self.screen.flush()
    }

    /// Enable Unicode core / grapheme-cluster mode (DECSET 2027) and flush,
    /// switching the screen to measure text per extended grapheme cluster so
    /// it agrees with the terminal.
    ///
    /// Only call this after [`capabilities`](Self::capabilities) reports
    /// [`supports(Mode::UNICODE_CORE)`](super::Capabilities::supports); a
    /// terminal that ignores the mode still measures per code point, and the
    /// two disagreeing misplaces every cell after the first cluster on a line.
    pub fn enable_grapheme_clusters(&mut self) -> io::Result<()> {
        mode::Mode::UNICODE_CORE.set(&mut self.screen)?;
        self.state.grapheme_clusters = true;
        self.screen.set_grapheme_clusters(true);
        self.screen.flush()
    }

    /// Disable grapheme-cluster mode (DECRST 2027) and flush, returning the
    /// screen to per-code-point (wcwidth-style) measurement.
    pub fn disable_grapheme_clusters(&mut self) -> io::Result<()> {
        mode::Mode::UNICODE_CORE.reset(&mut self.screen)?;
        self.state.grapheme_clusters = false;
        self.screen.set_grapheme_clusters(false);
        self.screen.flush()
    }

    /// Set the per-screen-buffer Kitty keyboard enhancements and flush.
    /// `Some(flags)` enables the selected progressive-enhancement bits;
    /// `None` disables every enhancement.
    ///
    /// The Kitty stack is per-screen-buffer, so the flags are re-applied on
    /// the newly active buffer by [`enter_alt_screen`](Self::enter_alt_screen)
    /// / [`exit_alt_screen`](Self::exit_alt_screen), as part of the switch.
    pub fn set_kitty_keyboard(
        &mut self,
        flags: Option<kitty::KittyKeyboardFlags>,
    ) -> io::Result<()> {
        let flags = flags.unwrap_or_else(kitty::KittyKeyboardFlags::empty);
        kitty::write_set_kitty_keyboard(&mut self.screen, flags, kitty::KittyKeyboardMode::Set)?;
        self.state.kitty_keyboard = flags;
        self.screen.flush()
    }

    /// Set the xterm modifyOtherKeys mode (`CSI > 4 ; n m`) and flush.
    /// Passing [`ModifyOtherKeysMode::Disabled`] resets it (`CSI > 4 m`).
    /// The mode is recorded so [`Program::finish`](super::Program::finish)
    /// can reset it and [`Program::resume`](super::Program::resume) re-apply
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
                self.screen.write_all(xterm::RESET_MODIFY_OTHER_KEYS)?
            }
            ModifyOtherKeysMode::Mode1 => self.screen.write_all(xterm::SET_MODIFY_OTHER_KEYS_1)?,
            ModifyOtherKeysMode::Mode2 => self.screen.write_all(xterm::SET_MODIFY_OTHER_KEYS_2)?,
        }
        self.state.modify_other_keys = mode;
        self.screen.flush()
    }

    /// Set the default foreground color (`OSC 10`) and flush. The color is
    /// converted to 24-bit RGB and emitted as `rgb:RRRR/GGGG/BBBB`, and is
    /// recorded so [`Program::finish`](super::Program::finish) can restore
    /// the terminal default and [`Program::resume`](super::Program::resume)
    /// can re-apply it.
    pub fn set_foreground_color(&mut self, color: Color) -> io::Result<()> {
        let (r, g, b) = color.to_rgb();
        color::write_set_foreground_color(&mut self.screen, &color::xparse_rgb(r, g, b))?;
        self.state.foreground_color = Some(color);
        self.screen.flush()
    }

    /// Restore the terminal's default foreground color (`OSC 110`) and
    /// flush.
    pub fn reset_foreground_color(&mut self) -> io::Result<()> {
        self.screen.write_all(color::RESET_FOREGROUND_COLOR)?;
        self.state.foreground_color = None;
        self.screen.flush()
    }

    /// Set the default background color (`OSC 11`) and flush. See
    /// [`set_foreground_color`](Self::set_foreground_color) for
    /// state-tracking semantics.
    pub fn set_background_color(&mut self, color: Color) -> io::Result<()> {
        let (r, g, b) = color.to_rgb();
        color::write_set_background_color(&mut self.screen, &color::xparse_rgb(r, g, b))?;
        self.state.background_color = Some(color);
        self.screen.flush()
    }

    /// Restore the terminal's default background color (`OSC 111`) and
    /// flush.
    pub fn reset_background_color(&mut self) -> io::Result<()> {
        self.screen.write_all(color::RESET_BACKGROUND_COLOR)?;
        self.state.background_color = None;
        self.screen.flush()
    }

    /// Set the cursor color (`OSC 12`) and flush. See
    /// [`set_foreground_color`](Self::set_foreground_color) for
    /// state-tracking semantics.
    pub fn set_cursor_color(&mut self, color: Color) -> io::Result<()> {
        let (r, g, b) = color.to_rgb();
        color::write_set_cursor_color(&mut self.screen, &color::xparse_rgb(r, g, b))?;
        self.state.cursor_color = Some(color);
        self.screen.flush()
    }

    /// Restore the terminal's default cursor color (`OSC 112`) and flush.
    pub fn reset_cursor_color(&mut self) -> io::Result<()> {
        self.screen.write_all(color::RESET_CURSOR_COLOR)?;
        self.state.cursor_color = None;
        self.screen.flush()
    }

    /// Set a terminal palette color by index (`OSC 4`) and flush. The
    /// override is tracked so [`Program::finish`](super::Program::finish) can
    /// restore it and [`Program::resume`](super::Program::resume) re-apply it.
    pub fn set_palette_color(&mut self, index: u8, color: Color) -> io::Result<()> {
        let (r, g, b) = color.to_rgb();
        color::write_set_palette_color(&mut self.screen, index, &color::xparse_rgb(r, g, b))?;
        self.state.palette.insert(index, color);
        self.screen.flush()
    }

    /// Reset a single terminal palette color to its default
    /// (`OSC 104 ; index`) and flush.
    pub fn reset_palette_color(&mut self, index: u8) -> io::Result<()> {
        color::write_reset_palette_color(&mut self.screen, index)?;
        self.state.palette.remove(&index);
        self.screen.flush()
    }

    /// Reset the entire terminal palette to its defaults (`OSC 104`) and
    /// flush, clearing every tracked palette override.
    pub fn reset_palette_colors(&mut self) -> io::Result<()> {
        self.screen.write_all(color::RESET_PALETTE_COLORS)?;
        self.state.palette.clear();
        self.screen.flush()
    }

    /// Stage the teardown of every mode currently held — non-render modes
    /// (cursor style, mouse, paste, focus, colors, title, …) followed by the
    /// render-coupled modes (cursor visibility, alternate screen, Kitty
    /// keyboard, Unicode core) — returning the terminal to a clean baseline
    /// before handing control back to the shell. Pure write — does not mutate
    /// the tracked state, so a later [`restore`](Self::restore) re-applies the
    /// same modes verbatim. The caller flushes.
    pub(super) fn reset(&mut self) -> io::Result<()> {
        // --- Non-render modes ---
        if self.state.cursor_style != cursor::CursorStyle::Default {
            cursor::write_cursor_style(&mut self.screen, cursor::CursorStyle::Default)?;
        }
        if self.state.bracketed_paste {
            mode::Mode::BRACKETED_PASTE.reset(&mut self.screen)?;
        }
        if self.state.focus_events {
            mode::Mode::FOCUS.reset(&mut self.screen)?;
        }
        if let Some(tracking) = self.state.mouse {
            self.write_mouse_modes(tracking, false)?;
        }
        if self.state.color_scheme_updates {
            mode::Mode::LIGHT_DARK.reset(&mut self.screen)?;
        }
        if self.state.in_band_resize {
            mode::Mode::IN_BAND_RESIZE.reset(&mut self.screen)?;
        }
        if self.state.modify_other_keys != crate::event::ModifyOtherKeysMode::Disabled {
            self.screen.write_all(xterm::RESET_MODIFY_OTHER_KEYS)?;
        }
        if self.state.foreground_color.is_some() {
            self.screen.write_all(color::RESET_FOREGROUND_COLOR)?;
        }
        if self.state.background_color.is_some() {
            self.screen.write_all(color::RESET_BACKGROUND_COLOR)?;
        }
        if self.state.cursor_color.is_some() {
            self.screen.write_all(color::RESET_CURSOR_COLOR)?;
        }
        if !self.state.palette.is_empty() {
            self.screen.write_all(color::RESET_PALETTE_COLORS)?;
        }
        match (&self.state.window_title, &self.state.icon_name) {
            // Both set to the same string (e.g. via `set_title`): clear both
            // with a single `OSC 0`.
            (Some(w), Some(i)) if w == i => {
                ansi::title::write_window_title_and_icon(&mut self.screen, "")?;
            }
            (window_title, icon_name) => {
                if window_title.is_some() {
                    ansi::title::write_window_title(&mut self.screen, "")?;
                }
                if icon_name.is_some() {
                    ansi::title::write_icon_name(&mut self.screen, "")?;
                }
            }
        }
        if self.state.pointer_shape.is_some() {
            cursor::write_set_pointer_shape(&mut self.screen, "default")?;
        }
        if self.state.progress.is_some() {
            self.screen.write_all(progress::RESET_PROGRESS_BAR)?;
        }

        // --- Render-coupled modes ---
        // Park the cursor below the managed area before any teardown, so the
        // shell prompt lands where the user expects.
        self.screen.park_cursor()?;
        if !self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.set(&mut self.screen)?;
        }
        let fullscreen = self.state.alt_screen;
        // Clear the alt screen's kitty keyboard frame *before* leaving the alt
        // screen — the stack is per-screen-buffer.
        if fullscreen && !self.state.kitty_keyboard.is_empty() {
            kitty::write_set_kitty_keyboard(
                &mut self.screen,
                kitty::KittyKeyboardFlags::empty(),
                kitty::KittyKeyboardMode::Set,
            )?;
        }
        if fullscreen {
            mode::Mode::ALT_SCREEN_SAVE_CURSOR.reset(&mut self.screen)?;
            self.screen.restore_cursor();
        }
        // Now on the main screen — clear its frame too.
        if !self.state.kitty_keyboard.is_empty() {
            kitty::write_set_kitty_keyboard(
                &mut self.screen,
                kitty::KittyKeyboardFlags::empty(),
                kitty::KittyKeyboardMode::Set,
            )?;
        }
        if self.state.grapheme_clusters {
            mode::Mode::UNICODE_CORE.reset(&mut self.screen)?;
        }
        self.screen.invalidate_cursor();
        Ok(())
    }

    /// Re-emit every mode held in the tracked state — the render-coupled
    /// modes (Kitty keyboard, alternate screen, Unicode core, cursor
    /// visibility) first, then the non-render modes — for any scenario where
    /// the terminal was temporarily handed back to the shell. Pairs with
    /// [`reset`](Self::reset). Pure write — does not mutate the tracked state.
    /// The caller flushes.
    pub(super) fn restore(&mut self) -> io::Result<()> {
        // --- Render-coupled modes ---
        // Re-apply the desired kitty keyboard flags on the main screen
        // *before* entering the alt screen — the stack is per-buffer.
        if !self.state.kitty_keyboard.is_empty() {
            kitty::write_set_kitty_keyboard(
                &mut self.screen,
                self.state.kitty_keyboard,
                kitty::KittyKeyboardMode::Set,
            )?;
        }
        let fullscreen = self.state.alt_screen;
        if fullscreen {
            self.screen.save_cursor();
            mode::Mode::ALT_SCREEN_SAVE_CURSOR.set(&mut self.screen)?;
        }
        // Now on the alt screen (if alt was active) — re-apply on the alt
        // buffer too, since its stack is independent.
        if fullscreen && !self.state.kitty_keyboard.is_empty() {
            kitty::write_set_kitty_keyboard(
                &mut self.screen,
                self.state.kitty_keyboard,
                kitty::KittyKeyboardMode::Set,
            )?;
        }
        if self.state.grapheme_clusters {
            mode::Mode::UNICODE_CORE.set(&mut self.screen)?;
        }
        if !self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.reset(&mut self.screen)?;
        }

        // --- Non-render modes ---
        if self.state.cursor_style != cursor::CursorStyle::Default {
            cursor::write_cursor_style(&mut self.screen, self.state.cursor_style)?;
        }
        if self.state.color_scheme_updates {
            mode::Mode::LIGHT_DARK.set(&mut self.screen)?;
        }
        if self.state.in_band_resize {
            mode::Mode::IN_BAND_RESIZE.set(&mut self.screen)?;
        }
        match self.state.modify_other_keys {
            crate::event::ModifyOtherKeysMode::Mode1 => {
                self.screen.write_all(xterm::SET_MODIFY_OTHER_KEYS_1)?;
            }
            crate::event::ModifyOtherKeysMode::Mode2 => {
                self.screen.write_all(xterm::SET_MODIFY_OTHER_KEYS_2)?;
            }
            crate::event::ModifyOtherKeysMode::Disabled => {}
        }
        if self.state.bracketed_paste {
            mode::Mode::BRACKETED_PASTE.set(&mut self.screen)?;
        }
        if self.state.focus_events {
            mode::Mode::FOCUS.set(&mut self.screen)?;
        }
        if let Some(pref) = self.state.mouse {
            self.write_mouse_modes(pref, true)?;
        }
        if let Some(c) = self.state.foreground_color {
            let (r, g, b) = c.to_rgb();
            color::write_set_foreground_color(&mut self.screen, &color::xparse_rgb(r, g, b))?;
        }
        if let Some(c) = self.state.background_color {
            let (r, g, b) = c.to_rgb();
            color::write_set_background_color(&mut self.screen, &color::xparse_rgb(r, g, b))?;
        }
        if let Some(c) = self.state.cursor_color {
            let (r, g, b) = c.to_rgb();
            color::write_set_cursor_color(&mut self.screen, &color::xparse_rgb(r, g, b))?;
        }
        for (&index, &c) in &self.state.palette {
            let (r, g, b) = c.to_rgb();
            color::write_set_palette_color(&mut self.screen, index, &color::xparse_rgb(r, g, b))?;
        }
        match (
            self.state.window_title.clone(),
            self.state.icon_name.clone(),
        ) {
            // Both set to the same string (e.g. via `set_title`): restore both
            // with a single `OSC 0`.
            (Some(w), Some(i)) if w == i => {
                ansi::title::write_window_title_and_icon(&mut self.screen, &w)?;
            }
            (window_title, icon_name) => {
                if let Some(title) = window_title {
                    ansi::title::write_window_title(&mut self.screen, &title)?;
                }
                if let Some(name) = icon_name {
                    ansi::title::write_icon_name(&mut self.screen, &name)?;
                }
            }
        }
        if let Some(shape) = self.state.pointer_shape.clone() {
            cursor::write_set_pointer_shape(&mut self.screen, &shape)?;
        }
        if let Some(progress) = self.state.progress {
            progress.write(&mut self.screen)?;
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
        self.screen
            .write_all(crate::ansi::winop::REQUEST_WINDOW_PIXEL_SIZE)?;
        self.screen.flush()
    }

    /// Request the character cell size in pixels (XTWINOPS `CSI 16 t`).
    /// Reply: [`Event::CellPixelSize`](crate::event::Event::CellPixelSize).
    pub fn request_cell_pixel_size(&mut self) -> io::Result<()> {
        self.screen
            .write_all(crate::ansi::winop::REQUEST_CELL_PIXEL_SIZE)?;
        self.screen.flush()
    }

    /// Request the physical screen coordinate of the managed area's top-left
    /// cell: park the cursor there and ask the terminal where it landed
    /// (`CSI 6n`).
    ///
    /// The reply arrives asynchronously as a
    /// [`CursorPosition`](crate::event::Event::CursorPosition) event and is
    /// recorded (and clipped to keep the managed area on screen) by
    /// [`observe_event`](Self::observe_event); read the result with
    /// [`origin`](Self::origin). The event is still delivered to you.
    ///
    /// Call this once mouse mapping starts, again whenever the terminal
    /// resizes, and again after [`resume`](Self::resume), each of which can
    /// move the managed area. Without it the origin stays at `(0, 0)` and
    /// [`mouse_to_origin`](Self::mouse_to_origin) is an identity.
    ///
    /// A no-op in fullscreen, where the origin is always `(0, 0)`.
    pub fn request_origin(&mut self) -> io::Result<()> {
        if self.screen.fullscreen() {
            return Ok(());
        }
        // Park the cursor at the surface top-left; in relative-cursor inline
        // mode that is the physical origin, so the reply *is* the origin.
        // Stage the move rather than using move_cursor_to, which would flush
        // before the query is written.
        self.screen
            .stage_move_cursor_to(crate::layout::Position::ORIGIN);
        self.screen
            .write_all(crate::ansi::status::REQUEST_CURSOR_POSITION)?;
        self.origin_queries_pending = self.origin_queries_pending.saturating_add(1);
        self.screen.flush()
    }

    /// Request the terminal's active Kitty keyboard flags (`CSI ? u`).
    /// The reply is recorded in [`capabilities`](Self::capabilities).
    pub fn request_kitty_keyboard(&mut self) -> io::Result<()> {
        self.screen
            .write_all(crate::ansi::kitty::REQUEST_KITTY_KEYBOARD)?;
        self.screen.flush()
    }

    /// Request the terminal's modifyOtherKeys state (`CSI ? 4 m`). The
    /// reply is recorded in [`capabilities`](Self::capabilities).
    pub fn request_modify_other_keys(&mut self) -> io::Result<()> {
        self.screen
            .write_all(crate::ansi::xterm::QUERY_MODIFY_OTHER_KEYS)?;
        self.screen.flush()
    }

    /// Request the default foreground color (`OSC 10 ; ? ST`). Reply:
    /// [`Event::ForegroundColor`](crate::event::Event::ForegroundColor).
    pub fn request_foreground_color(&mut self) -> io::Result<()> {
        self.screen
            .write_all(crate::ansi::color::REQUEST_FOREGROUND_COLOR)?;
        self.screen.flush()
    }

    /// Request the default background color (`OSC 11 ; ? ST`). Reply:
    /// [`Event::BackgroundColor`](crate::event::Event::BackgroundColor).
    pub fn request_background_color(&mut self) -> io::Result<()> {
        self.screen
            .write_all(crate::ansi::color::REQUEST_BACKGROUND_COLOR)?;
        self.screen.flush()
    }

    /// Request the cursor color (`OSC 12 ; ? ST`). Reply:
    /// [`Event::CursorColor`](crate::event::Event::CursorColor).
    pub fn request_cursor_color(&mut self) -> io::Result<()> {
        self.screen
            .write_all(crate::ansi::color::REQUEST_CURSOR_COLOR)?;
        self.screen.flush()
    }

    /// Request a terminal palette color by index (`OSC 4 ; index ; ? ST`).
    /// Reply: `OSC 4 ; index ; rgb:... ST`.
    pub fn request_palette_color(&mut self, index: u8) -> io::Result<()> {
        crate::ansi::color::write_request_palette_color(&mut self.screen, index)?;
        self.screen.flush()
    }

    /// Request a terminal mode's current setting (DECRQM). Reply:
    /// [`Event::ModeReport`](crate::event::Event::ModeReport).
    ///
    /// The reply's [`ModeSetting`](crate::ansi::mode::ModeSetting) reports whether
    /// the mode is set, reset, or permanently fixed. A permanently reset mode
    /// is recognized but can never be enabled, so check
    /// [`ModeSetting::is_available`](crate::ansi::mode::ModeSetting::is_available)
    /// before relying on it.
    pub fn request_mode(&mut self, mode: crate::ansi::mode::Mode) -> io::Result<()> {
        mode.request(&mut self.screen)?;
        self.screen.flush()
    }

    /// Request the cursor position (`CSI 6 n`). Reply:
    /// [`Event::CursorPosition`](crate::event::Event::CursorPosition).
    pub fn request_cursor_position(&mut self) -> io::Result<()> {
        self.screen
            .write_all(crate::ansi::status::REQUEST_CURSOR_POSITION)?;
        self.screen.flush()
    }

    /// Request the current color scheme (`CSI ? 996 n`): whether the
    /// terminal's scheme is dark or light. This reports only the dark/light
    /// preference, not the actual colors. Reply:
    /// [`Event::ColorScheme`](crate::event::Event::ColorScheme).
    pub fn request_color_scheme(&mut self) -> io::Result<()> {
        self.screen
            .write_all(crate::ansi::status::REQUEST_LIGHT_DARK_REPORT)?;
        self.screen.flush()
    }

    /// Set the system clipboard contents (`OSC 52 ; c`). `data` is
    /// base64-encoded for transport.
    pub fn set_system_clipboard(&mut self, data: &[u8]) -> io::Result<()> {
        crate::ansi::clipboard::write_set_clipboard(
            &mut self.screen,
            crate::ansi::clipboard::SYSTEM_CLIPBOARD,
            data,
        )?;
        self.screen.flush()
    }

    /// Set the primary selection contents (`OSC 52 ; p`). `data` is
    /// base64-encoded for transport.
    pub fn set_primary_clipboard(&mut self, data: &[u8]) -> io::Result<()> {
        crate::ansi::clipboard::write_set_clipboard(
            &mut self.screen,
            crate::ansi::clipboard::PRIMARY_CLIPBOARD,
            data,
        )?;
        self.screen.flush()
    }

    /// Request the system clipboard contents (`OSC 52 ; c ; ?`). Reply:
    /// [`Event::Clipboard`](crate::event::Event::Clipboard).
    pub fn request_system_clipboard(&mut self) -> io::Result<()> {
        crate::ansi::clipboard::write_request_clipboard(
            &mut self.screen,
            crate::ansi::clipboard::SYSTEM_CLIPBOARD,
        )?;
        self.screen.flush()
    }

    /// Request the primary selection contents (`OSC 52 ; p ; ?`). Reply:
    /// [`Event::Clipboard`](crate::event::Event::Clipboard).
    pub fn request_primary_clipboard(&mut self) -> io::Result<()> {
        crate::ansi::clipboard::write_request_clipboard(
            &mut self.screen,
            crate::ansi::clipboard::PRIMARY_CLIPBOARD,
        )?;
        self.screen.flush()
    }
}

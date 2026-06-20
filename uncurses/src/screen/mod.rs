//! [`Screen`] — a self-managing terminal application facade.
//!
//! `Screen<I, O>` bundles the three primitives a full-screen terminal
//! program needs into one owned handle:
//!
//! - a [`Terminal`] for the raw-mode lifecycle,
//! - a [`Canvas`] for cell-diffed rendering, and
//! - an [`EventSource`] for decoded input (read synchronously).
//!
//! It additionally owns the non-render terminal/input modes (mouse,
//! bracketed paste, focus reporting, in-band resize, the default
//! foreground/background/cursor colors, the window title, the cursor
//! style, and color-scheme update reports) and tracks them so they can be
//! torn down on a shell handoff and re-applied afterwards.
//!
//! Construction is inert: [`Screen::new`] (and the [`stdio`](Screen::stdio)
//! / [`open`](Screen::open) shortcuts) only build the screen. Begin a
//! session with [`Screen::init`], which enters raw mode and stages the
//! capability queries. Teardown is explicit: there is **no** `Drop`.
//! Hand the terminal back to the shell with [`Screen::finish`] (consume),
//! [`Screen::pause`] (keep, e.g. to shell out), or [`Screen::suspend`]
//! (pause, then stop the process with `SIGTSTP`); resume a
//! paused/suspended screen with [`Screen::resume`].
//!
//! ```no_run
//! use uncurses::screen::Screen;
//! use uncurses::style::Style;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut screen = Screen::open()?; // build over /dev/tty
//! screen.init()?; // raw mode + staged queries
//! screen.enter_alt_screen()?;
//! screen.set_str((0, 0), "hello", Style::default());
//! screen.present()?;
//! let _event = screen.read()?;
//! screen.finish()?; // restore the terminal
//! # Ok(())
//! # }
//! ```
//!
//! [`Terminal`]: crate::terminal::Terminal
//! [`Canvas`]: crate::canvas::Canvas
//! [`EventSource`]: crate::event::EventSource

mod cursor;
mod modes;
mod mouse;
mod state;

pub use cursor::CursorShape;
pub use state::Capabilities;

use std::io::{self, Write};
use std::time::Duration;

use crate::buffer::{Bounded, Surface, SurfaceMut};
use crate::canvas::Canvas;
use crate::cell::Cell;
use crate::event::source::Input;
use crate::event::{Event, EventSource};
use crate::layout::{Position, Rect, Size};
use crate::terminal::Terminal;
use crate::text::WrapMode;

/// A self-managing terminal application facade composing a [`Terminal`],
/// a [`Canvas`], and an [`EventSource`] with the non-render terminal and
/// input modes. See the [module documentation](self) for the lifecycle.
///
/// [`Terminal`]: crate::terminal::Terminal
/// [`Canvas`]: crate::canvas::Canvas
/// [`EventSource`]: crate::event::EventSource
pub struct Screen<I, O>
where
    I: Input,
    O: Write,
{
    terminal: Terminal<I, O>,
    canvas: Canvas<O>,
    /// Owned input source, read synchronously through [`Self::read`] and
    /// friends.
    source: EventSource<I>,
    state: state::State,
    /// Terminal capabilities detected by intercepting the replies to the
    /// queries [`Self::init`] fires. Capability-report events are absorbed
    /// by the event delegates and applied as side effects rather than
    /// surfaced to the caller.
    caps: Capabilities,
    /// Desired default behaviors, set by [`Self::init_with`].
    options: ScreenOptions,
    /// Set once the discovery-dependent defaults have been applied (on the
    /// terminating Primary DA reply), so they are applied at most once.
    defaults_applied: bool,
    /// Last observed full terminal size in cells, from resize and
    /// `WindowCellSize` reports. `None` until first observed.
    window_cells: Option<Size>,
    /// Last observed full terminal size in pixels, from resize (when it
    /// carries pixel dimensions) and `WindowPixelSize` reports. `None`
    /// until first observed.
    window_pixels: Option<Size>,
    /// The raw XTVERSION reply identifying the terminal (e.g.
    /// `"XTerm(380)"`). `None` until the reply is observed.
    terminal_name: Option<String>,
}

/// Desired default behaviors applied by [`Screen::init_with`].
///
/// Always-on defaults (e.g. [`bracketed_paste`](Self::bracketed_paste))
/// take effect at init regardless of capability detection. Discovery-driven
/// defaults are applied once the terminating Primary DA reply confirms the
/// detected [`Capabilities`]; if the terminal never answers, only the
/// always-on defaults are in effect.
#[derive(Debug, Clone)]
pub struct ScreenOptions {
    /// Enable bracketed paste at init. Defaults to `true`.
    pub bracketed_paste: bool,
    /// Desired Kitty keyboard enhancements. When non-empty, the screen
    /// enables as many as the terminal supports, preferring the Kitty
    /// protocol and falling back to xterm modifyOtherKeys when Kitty is
    /// unavailable. Defaults to
    /// [`KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES`].
    ///
    /// [`KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES`]: crate::ansi::KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
    pub keyboard_enhancements: crate::ansi::KittyKeyboardFlags,
    /// Prefer in-band resize reports over the `SIGWINCH` path when the
    /// terminal supports them. Defaults to `true`.
    pub prefer_in_band_resize: bool,
    /// Request the window pixel size (XTWINOPS `CSI 14 t`) whenever a resize
    /// is observed that does not itself carry pixel dimensions, keeping
    /// [`window_pixels`](Screen::window_pixels) current on platforms that
    /// report cell sizes only. Skipped while in-band resize is active, since
    /// those reports already carry pixel dimensions. Defaults to `true` on
    /// Windows (whose console resize events carry no pixel size) and `false`
    /// elsewhere, where resize reports already include pixel dimensions.
    pub request_pixel_size_on_resize: bool,
}

impl Default for ScreenOptions {
    fn default() -> Self {
        Self {
            bracketed_paste: true,
            keyboard_enhancements: crate::ansi::KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES,
            prefer_in_band_resize: true,
            request_pixel_size_on_resize: cfg!(windows),
        }
    }
}

impl<I, O> Screen<I, O>
where
    I: Input,
    O: Write,
{
    // --- Canvas drawing delegates ---------------------------------------

    /// Paint `s` into the canvas at `pos` with `style`. See
    /// [`Canvas::set_str`].
    pub fn set_str(
        &mut self,
        pos: impl Into<Position>,
        s: &str,
        style: crate::style::Style,
    ) -> Position {
        self.canvas.set_str(pos, s, style)
    }

    /// Like [`Self::set_str`] but with an explicit [`WrapMode`]. See
    /// [`Canvas::set_str_wrap`].
    pub fn set_str_wrap(
        &mut self,
        pos: impl Into<Position>,
        s: &str,
        wrap: WrapMode,
        style: crate::style::Style,
    ) -> Position {
        self.canvas.set_str_wrap(pos, s, wrap, style)
    }

    /// Paint `s` clipped to `rect` with `style`. See
    /// [`Canvas::set_str_rect`].
    pub fn set_str_rect(
        &mut self,
        rect: impl Into<crate::layout::Rect>,
        s: &str,
        style: crate::style::Style,
    ) -> Position {
        self.canvas.set_str_rect(rect, s, style)
    }

    /// Like [`Self::set_str_rect`] but with an explicit [`WrapMode`]. See
    /// [`Canvas::set_str_rect_wrap`].
    pub fn set_str_rect_wrap(
        &mut self,
        rect: impl Into<crate::layout::Rect>,
        s: &str,
        wrap: WrapMode,
        style: crate::style::Style,
    ) -> Position {
        self.canvas.set_str_rect_wrap(rect, s, wrap, style)
    }

    /// Write `cell` at `pos`. See [`Canvas::set_cell`].
    pub fn set_cell(&mut self, pos: impl Into<Position>, cell: &Cell) {
        self.canvas.set_cell(pos, cell);
    }

    /// Mutable access to the cell at `pos`. See [`Canvas::cell_mut`].
    pub fn cell_mut(&mut self, pos: impl Into<Position>) -> Option<&mut Cell> {
        self.canvas.cell_mut(pos)
    }

    /// Diff the staged frame and stage the escape bytes. See
    /// [`Canvas::render`].
    pub fn render(&mut self) {
        self.canvas.render();
    }

    /// Render then flush a complete frame. See [`Canvas::present`].
    pub fn present(&mut self) -> io::Result<()> {
        self.canvas.present()
    }

    /// Force a full redraw on the next render. See [`Canvas::invalidate`].
    pub fn invalidate(&mut self) {
        self.canvas.invalidate();
    }

    /// Resize the canvas. See [`Canvas::resize`].
    pub fn resize(&mut self, size: impl Into<Size>) {
        let size = size.into();
        self.canvas.resize(size.width, size.height);
    }

    /// Cache a fresh terminal size and request the pixel size over the wire
    /// when the size carried none (e.g. the Windows console, whose
    /// `get_window_size` reports no pixel dimensions), gated by
    /// [`request_pixel_size_on_resize`](ScreenOptions::request_pixel_size_on_resize)
    /// and the absence of in-band resize. The pixel reply arrives later as a
    /// [`WindowPixelSize`](crate::event::Event::WindowPixelSize) event.
    fn cache_window_size(&mut self, ws: crate::terminal::Winsize) -> io::Result<()> {
        self.window_cells = Some(Size::new(ws.col, ws.row));
        if ws.xpixel > 0 && ws.ypixel > 0 {
            self.window_pixels = Some(Size::new(ws.xpixel, ws.ypixel));
        } else if self.options.request_pixel_size_on_resize && !self.state.in_band_resize {
            self.request_window_pixel_size()?;
        }
        Ok(())
    }

    /// Insert `content` above the screen. See [`Canvas::insert_above`].
    pub fn insert_above(&mut self, content: &str) {
        self.canvas.insert_above(content);
    }

    /// The canvas size in cells.
    pub fn size(&self) -> Size {
        Size::new(self.canvas.width(), self.canvas.height())
    }

    /// Move the staged cursor to `pos`. See [`Canvas::set_cursor_position`].
    pub fn move_cursor_to(&mut self, pos: impl Into<Position>) {
        let pos = pos.into();
        self.canvas.set_cursor_position(pos.x, pos.y);
    }

    // --- Render-coupled mode delegates ----------------------------------

    /// Enter the alternate screen and flush. See [`Canvas::set_alt_screen`].
    pub fn enter_alt_screen(&mut self) -> io::Result<()> {
        self.canvas.set_alt_screen(true);
        self.canvas.flush()
    }

    /// Leave the alternate screen and flush. See [`Canvas::set_alt_screen`].
    pub fn exit_alt_screen(&mut self) -> io::Result<()> {
        self.canvas.set_alt_screen(false);
        self.canvas.flush()
    }

    /// Show the cursor and flush. See [`Canvas::set_cursor_visible`].
    pub fn show_cursor(&mut self) -> io::Result<()> {
        self.canvas.set_cursor_visible(true);
        self.canvas.flush()
    }

    /// Hide the cursor and flush. See [`Canvas::set_cursor_visible`].
    pub fn hide_cursor(&mut self) -> io::Result<()> {
        self.canvas.set_cursor_visible(false);
        self.canvas.flush()
    }

    /// Set the per-screen kitty keyboard enhancements and flush.
    /// `Some(flags)` enables the selected progressive-enhancement bits;
    /// `None` disables every enhancement. See
    /// [`Canvas::set_kitty_keyboard_flags`].
    pub fn set_kitty_keyboard(
        &mut self,
        flags: Option<crate::ansi::KittyKeyboardFlags>,
    ) -> io::Result<()> {
        let flags = flags.unwrap_or(crate::ansi::KittyKeyboardFlags::NONE);
        self.canvas.set_kitty_keyboard_flags(flags);
        self.canvas.flush()
    }

    /// Set the color profile used when emitting styled cells. See
    /// [`Canvas::use_color_profile`].
    pub fn use_color_profile(&mut self, profile: crate::color::Profile) {
        self.canvas.use_color_profile(profile);
    }

    /// Set the renderer optimization flags. See
    /// [`Canvas::use_optimizations`].
    pub fn use_optimizations(&mut self, optimizations: crate::canvas::Optimizations) {
        self.canvas.use_optimizations(optimizations);
    }

    // --- Event delegates -------------------------------------------------

    /// Drive the input source for up to `timeout`, returning whether any
    /// event became available. See [`EventSource::poll`].
    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        self.source.poll(timeout)
    }

    /// Take the next queued event without doing I/O. Capability reports are
    /// recorded as a side effect but still returned. See
    /// [`EventSource::try_read`].
    pub fn try_read(&mut self) -> Option<Event> {
        let ev = self.source.try_read()?;
        // A failed flush while applying discovery-driven defaults is
        // best-effort here; it resurfaces on the next explicit flush.
        let _ = self.observe(&ev);
        Some(ev)
    }

    /// Block until the next event. Capability reports are recorded as a
    /// side effect but still returned. See [`EventSource::read`].
    pub fn read(&mut self) -> io::Result<Event> {
        let ev = self.source.read()?;
        self.observe(&ev)?;
        Ok(ev)
    }

    /// Terminal capabilities detected so far from intercepted query
    /// replies. Populated as the relevant reports arrive through the event
    /// delegates after [`Self::init`].
    pub fn capabilities(&self) -> Capabilities {
        self.caps
    }

    /// Last observed full terminal size in cells, cached from resize and
    /// `WindowCellSize` reports as they flow through the event delegates.
    /// `None` until one has been observed.
    pub fn window_cells(&self) -> Option<Size> {
        self.window_cells
    }

    /// Last observed full terminal size in pixels, cached from resize
    /// (when it carries pixel dimensions) and from
    /// [`request_window_pixel_size`](Self::request_window_pixel_size)
    /// replies. `None` until one has been observed.
    pub fn window_pixels(&self) -> Option<Size> {
        self.window_pixels
    }

    /// The raw XTVERSION reply identifying the terminal (e.g.
    /// `"XTerm(380)"`). `None` until the reply has been observed.
    pub fn terminal_name(&self) -> Option<&str> {
        self.terminal_name.as_deref()
    }

    /// Convert a [`Mouse`](crate::event::Mouse) event reported in pixel
    /// coordinates (SGR-pixel encoding) to cell coordinates using the
    /// cached terminal size. Returns `None` when the window pixel size has
    /// not been observed yet, so no conversion is possible — request it
    /// with [`request_window_pixel_size`](Self::request_window_pixel_size),
    /// or rely on an in-band resize report to populate it.
    pub fn mouse_pixels_to_cells(&self, mouse: crate::event::Mouse) -> Option<crate::event::Mouse> {
        let pixels = self.window_pixels?;
        let cells = self.window_cells.unwrap_or_else(|| self.size());
        Some(crate::event::mouse_pixel_to_cell(
            mouse,
            pixels.width,
            pixels.height,
            cells.width,
            cells.height,
        ))
    }

    /// Observe an event as it passes to the caller. Capability-report
    /// replies to the queries [`Self::init`] fires are recorded, and the
    /// render-affecting ones applied; the event is never consumed. On the
    /// terminating Primary DA reply, the discovery-driven defaults from the
    /// active [`ScreenOptions`] are applied (once).
    fn observe(&mut self, event: &Event) -> io::Result<()> {
        use crate::ansi::mode::Mode;
        match *event {
            Event::ModeReport { mode, setting } if setting.is_recognized() => match mode {
                // Render-affecting: record and apply.
                Mode::SYNCHRONIZED_OUTPUT => {
                    self.caps.synchronized_output = true;
                    self.canvas.set_sync_updates(true);
                }
                Mode::UNICODE_CORE => {
                    self.caps.grapheme_clusters = true;
                    self.canvas.set_grapheme_clusters(true);
                }
                // Recorded only; enabling is the app's choice.
                Mode::BRACKETED_PASTE => self.caps.bracketed_paste = true,
                Mode::LIGHT_DARK => self.caps.color_theme_updates = true,
                Mode::IN_BAND_RESIZE => self.caps.in_band_resize = true,
                Mode::FOCUS => self.caps.focus_events = true,
                Mode::MOUSE_NORMAL => self.caps.mouse_normal = true,
                Mode::MOUSE_BUTTON => self.caps.mouse_button = true,
                Mode::MOUSE_ANY => self.caps.mouse_any = true,
                Mode::MOUSE_SGR => self.caps.mouse_sgr = true,
                Mode::MOUSE_SGR_PIXEL => self.caps.mouse_sgr_pixel = true,
                _ => {}
            },
            Event::KittyKeyboardEnhancements(_) => self.caps.kitty_keyboard = true,
            // Any modifyOtherKeys report (`CSI > 4 ; n m`) answers our
            // query, so a reply means the terminal recognizes the feature.
            Event::ModifyOtherKeys(_) => self.caps.modify_other_keys = true,
            Event::PrimaryDeviceAttributes(ref attrs) => {
                if attrs.contains(&Some(4)) {
                    self.caps.sixel = true;
                }
                if attrs.contains(&Some(52)) {
                    self.caps.clipboard = true;
                }
                // Primary DA is the terminating reply: every capability is
                // now known, so apply the discovery-driven defaults once.
                if !self.defaults_applied {
                    self.defaults_applied = true;
                    self.apply_defaults()?;
                }
            }
            Event::TerminalName(ref report) => {
                self.terminal_name = Some(report.clone());
            }
            // Cache the full terminal size as it changes. Refitting the
            // canvas is left to the app (call autoresize() as desired).
            Event::Resize(ws) => {
                self.cache_window_size(ws)?;
            }
            Event::WindowCellSize { width, height } => {
                self.window_cells = Some(Size::new(width, height));
            }
            Event::WindowPixelSize { width, height } => {
                self.window_pixels = Some(Size::new(width, height));
            }
            // A successful XTGETTCAP reply for a truecolor capability
            // confirms direct-color support: record and upgrade the
            // renderer's color profile.
            Event::Termcap {
                recognized: true,
                ref payload,
            } if payload.contains("RGB") || payload.contains("Tc") => {
                self.caps.true_color = true;
                self.canvas
                    .use_color_profile(crate::color::Profile::TrueColor);
            }
            _ => {}
        }
        Ok(())
    }

    /// Apply the discovery-driven defaults from the active [`ScreenOptions`]
    /// once every capability is known (called on the Primary DA reply).
    fn apply_defaults(&mut self) -> io::Result<()> {
        use crate::event::ModifyOtherKeysMode;

        // Prefer in-band resize over the SIGWINCH path when supported.
        if self.options.prefer_in_band_resize && self.caps.in_band_resize {
            self.enable_in_band_resize()?;
            self.source.set_handle_resize(false);
        }

        // Keyboard enhancements: prefer the Kitty protocol, falling back to
        // xterm modifyOtherKeys, enabling only what the terminal supports.
        if !self.options.keyboard_enhancements.is_empty() {
            if self.caps.kitty_keyboard {
                self.set_kitty_keyboard(Some(self.options.keyboard_enhancements))?;
            } else if self.caps.modify_other_keys {
                self.set_modify_other_keys(ModifyOtherKeysMode::Mode2)?;
            }
        }
        Ok(())
    }

    /// Whether the host is Apple's `Terminal.app`, which does not support
    /// most of the queried features and mishandles the queries themselves.
    fn is_apple_terminal(&self) -> bool {
        self.terminal.get_env("TERM_PROGRAM").as_deref() == Some("Apple_Terminal")
    }

    /// Stage the initial capability queries into the output stream. Their
    /// replies arrive asynchronously through the normal event flow and are
    /// intercepted by the event delegates (see [`Self::intercept`]).
    ///
    /// The mode (DECRQM), XTVERSION, and XTGETTCAP queries are skipped on
    /// Apple's `Terminal.app`, which mishandles them. A Primary DA request
    /// is sent last so its reply marks the end of the capability replies.
    fn stage_init_queries(&mut self) -> io::Result<()> {
        use crate::ansi::ctrl::{REQUEST_PRIMARY_DA, REQUEST_XTVERSION};
        use crate::ansi::kitty::REQUEST_KITTY_KEYBOARD;
        use crate::ansi::mode::Mode;
        use crate::ansi::termcap::write_xtgettcap;
        use crate::color::Profile;

        // Detect the env-derived profile, apply it to the renderer, and
        // remember whether there is headroom to upgrade via XTGETTCAP.
        let profile = Profile::detect_from(self.terminal.env(), true);
        self.canvas.use_color_profile(profile);

        // Always-safe queries.
        self.canvas.write_all(REQUEST_KITTY_KEYBOARD)?;

        if !self.is_apple_terminal() {
            for mode in [
                Mode::SYNCHRONIZED_OUTPUT,
                Mode::UNICODE_CORE,
                Mode::BRACKETED_PASTE,
                Mode::LIGHT_DARK,
                Mode::IN_BAND_RESIZE,
                Mode::FOCUS,
                Mode::MOUSE_NORMAL,
                Mode::MOUSE_BUTTON,
                Mode::MOUSE_ANY,
                Mode::MOUSE_SGR,
                Mode::MOUSE_SGR_PIXEL,
            ] {
                mode.request(&mut self.canvas)?;
            }
            self.canvas.write_all(REQUEST_XTVERSION)?;
            self.canvas
                .write_all(crate::ansi::xterm::QUERY_MODIFY_OTHER_KEYS)?;
            if profile < Profile::TrueColor {
                // One key per query: some terminals only answer the first
                // capability when several are batched in a single request.
                write_xtgettcap(&mut self.canvas, &["RGB"])?;
                write_xtgettcap(&mut self.canvas, &["Tc"])?;
            }
        } else {
            // Terminal.app mishandles the capability queries, but its
            // support for these features is known, so record them directly:
            // bracketed paste, mouse tracking (X10/normal/button/any), and
            // the X10/SGR encodings (no pixel reporting).
            self.caps.bracketed_paste = true;
            self.caps.mouse_normal = true;
            self.caps.mouse_button = true;
            self.caps.mouse_any = true;
            self.caps.mouse_sgr = true;
        }

        self.canvas.write_all(REQUEST_PRIMARY_DA)?;
        self.canvas.flush()
    }
}

impl<I, O> Write for Screen<I, O>
where
    I: Input,
    O: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.canvas.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.canvas.flush()
    }
}

impl<I, O> Bounded for Screen<I, O>
where
    I: Input,
    O: Write,
{
    fn bounds(&self) -> Rect {
        self.canvas.bounds()
    }
}

impl<I, O> Surface for Screen<I, O>
where
    I: Input,
    O: Write,
{
    fn cell(&self, pos: Position) -> Option<&Cell> {
        self.canvas.cell(pos)
    }
}

impl<I, O> SurfaceMut for Screen<I, O>
where
    I: Input,
    O: Write,
{
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        self.canvas.set_cell(pos, cell);
    }

    fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell> {
        self.canvas.cell_mut(pos)
    }

    fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.canvas.insert_lines(y, n, bounds_bottom, fill);
    }

    fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.canvas.delete_lines(y, n, bounds_bottom, fill);
    }

    fn insert_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        self.canvas.insert_cells(pos, n, bounds_right, fill);
    }

    fn delete_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        self.canvas.delete_cells(pos, n, bounds_right, fill);
    }
}

#[cfg(unix)]
impl<I, O> Screen<I, O>
where
    I: Input + Copy + std::os::fd::AsFd,
    O: Write + Copy + std::os::fd::AsFd,
{
    /// Construct a screen over `terminal` without touching the terminal:
    /// size a [`Canvas`] to it and create an [`EventSource`] on its input
    /// half. The terminal is left as-is; call [`Self::init`] to enter raw
    /// mode and begin a session.
    pub fn new(terminal: Terminal<I, O>) -> io::Result<Self> {
        let canvas = Canvas::from_terminal(&terminal)?;
        let source = EventSource::new(terminal.input())?;
        Ok(Self {
            terminal,
            canvas,
            source,
            state: state::State::default(),
            caps: Capabilities::default(),
            options: ScreenOptions::default(),
            defaults_applied: false,
            window_cells: None,
            window_pixels: None,
            terminal_name: None,
        })
    }

    /// Begin a session with the default [`ScreenOptions`]. See
    /// [`Self::init_with`].
    pub fn init(&mut self) -> io::Result<()> {
        self.init_with(ScreenOptions::default())
    }

    /// Begin a session: enter raw mode, apply the always-on defaults from
    /// `options`, and stage the capability queries whose replies the event
    /// loop consumes. Discovery-driven defaults are applied later, once the
    /// terminating Primary DA reply confirms the detected capabilities (see
    /// [`Self::capabilities`]). Call once after [`Self::new`], before
    /// rendering.
    pub fn init_with(&mut self, options: ScreenOptions) -> io::Result<()> {
        self.options = options;
        self.terminal.make_raw()?;
        self.autoresize()?;
        if self.options.bracketed_paste {
            self.enable_bracketed_paste()?;
        }
        self.stage_init_queries()
    }

    /// Query the current terminal window size (output half first, input as
    /// fallback). This is a live query; the cached
    /// [`window_cells`](Self::window_cells) /
    /// [`window_pixels`](Self::window_pixels) accessors return the
    /// last-observed values without I/O.
    pub fn get_window_size(&self) -> io::Result<crate::terminal::Winsize> {
        self.terminal.get_window_size()
    }

    /// Re-query the terminal size and resize the canvas to fit: the full
    /// terminal size in fullscreen (alternate screen on), or the terminal
    /// width with the current canvas height preserved inline (alternate
    /// screen off). Refreshes the cached [`window_cells`](Self::window_cells)
    /// / [`window_pixels`](Self::window_pixels); on platforms whose size
    /// query reports no pixel size (e.g. the Windows console) the pixel size
    /// is requested over the wire.
    pub fn autoresize(&mut self) -> io::Result<()> {
        let Ok(ws) = self.terminal.get_window_size() else {
            // Keep the current size when the query fails rather than
            // collapsing the canvas to zero.
            return Ok(());
        };
        self.cache_window_size(ws)?;
        let height = if self.canvas.alt_screen() {
            ws.row
        } else {
            self.canvas.height()
        };
        self.canvas.resize(ws.col, height);
        Ok(())
    }

    /// Consume the screen and hand the terminal back to the shell: tear
    /// down every staged mode, reset the canvas, flush, and restore the
    /// terminal's prior state.
    pub fn finish(mut self) -> io::Result<()> {
        self.stage_teardown()?;
        self.terminal.restore()
    }

    /// Hand the terminal back to the shell without consuming the screen,
    /// e.g. to run a child process. Re-enter with [`Self::resume`]. Like
    /// [`Self::finish`] but keeps the screen.
    pub fn pause(&mut self) -> io::Result<()> {
        self.stage_teardown()?;
        self.terminal.restore()
    }

    /// Re-acquire the terminal after a [`Self::pause`] or [`Self::suspend`]:
    /// re-enter raw mode, refit the canvas to the current viewport, re-apply
    /// the saved render state and modes, and force a full repaint.
    pub fn resume(&mut self) -> io::Result<()> {
        self.terminal.make_raw()?;
        self.autoresize()?;
        self.canvas.restore();
        self.restore_modes()?;
        self.canvas.invalidate();
        self.canvas.flush()
    }

    /// Suspend the process: [`pause`](Self::pause) the screen, then stop
    /// the process with `SIGTSTP`. Returns once the process is
    /// foregrounded again; the caller should then call [`Self::resume`].
    pub fn suspend(&mut self) -> io::Result<()> {
        self.pause()?;
        // SAFETY: raise is async-signal-safe.
        unsafe { libc::raise(libc::SIGTSTP) };
        Ok(())
    }

    fn stage_teardown(&mut self) -> io::Result<()> {
        self.reset_modes()?;
        self.canvas.reset();
        self.canvas.flush()
    }
}

#[cfg(windows)]
impl<I, O> Screen<I, O>
where
    I: Input + Copy + std::os::windows::io::AsHandle,
    O: Write + Copy + std::os::windows::io::AsHandle,
{
    /// Construct a screen over `terminal` without touching the terminal:
    /// size a [`Canvas`] to it and create an [`EventSource`] on its input
    /// half. The terminal is left as-is; call [`Self::init`] to enter raw
    /// mode and begin a session.
    pub fn new(terminal: Terminal<I, O>) -> io::Result<Self> {
        let canvas = Canvas::from_terminal(&terminal)?;
        let source = EventSource::new(terminal.input())?;
        Ok(Self {
            terminal,
            canvas,
            source,
            state: state::State::default(),
            caps: Capabilities::default(),
            options: ScreenOptions::default(),
            defaults_applied: false,
            window_cells: None,
            window_pixels: None,
            terminal_name: None,
        })
    }

    /// Begin a session with the default [`ScreenOptions`]. See
    /// [`Self::init_with`].
    pub fn init(&mut self) -> io::Result<()> {
        self.init_with(ScreenOptions::default())
    }

    /// Begin a session: enter raw mode, apply the always-on defaults from
    /// `options`, and stage the capability queries whose replies the event
    /// loop consumes. Discovery-driven defaults are applied later, once the
    /// terminating Primary DA reply confirms the detected capabilities (see
    /// [`Self::capabilities`]). Call once after [`Self::new`], before
    /// rendering.
    pub fn init_with(&mut self, options: ScreenOptions) -> io::Result<()> {
        self.options = options;
        self.terminal.make_raw()?;
        self.autoresize()?;
        if self.options.bracketed_paste {
            self.enable_bracketed_paste()?;
        }
        self.stage_init_queries()
    }

    /// Query the current terminal window size (output half first, input as
    /// fallback). This is a live query; the cached
    /// [`window_cells`](Self::window_cells) /
    /// [`window_pixels`](Self::window_pixels) accessors return the
    /// last-observed values without I/O.
    pub fn get_window_size(&self) -> io::Result<crate::terminal::Winsize> {
        self.terminal.get_window_size()
    }

    /// Re-query the terminal size and resize the canvas to fit: the full
    /// terminal size in fullscreen (alternate screen on), or the terminal
    /// width with the current canvas height preserved inline (alternate
    /// screen off). Refreshes the cached [`window_cells`](Self::window_cells)
    /// / [`window_pixels`](Self::window_pixels); on platforms whose size
    /// query reports no pixel size (e.g. the Windows console) the pixel size
    /// is requested over the wire.
    pub fn autoresize(&mut self) -> io::Result<()> {
        let Ok(ws) = self.terminal.get_window_size() else {
            // Keep the current size when the query fails rather than
            // collapsing the canvas to zero.
            return Ok(());
        };
        self.cache_window_size(ws)?;
        let height = if self.canvas.alt_screen() {
            ws.row
        } else {
            self.canvas.height()
        };
        self.canvas.resize(ws.col, height);
        Ok(())
    }

    /// Consume the screen and hand the terminal back to the shell: tear
    /// down every staged mode, reset the canvas, flush, and restore the
    /// terminal's prior state.
    pub fn finish(mut self) -> io::Result<()> {
        self.stage_teardown()?;
        self.terminal.restore()
    }

    /// Hand the terminal back to the shell without consuming the screen,
    /// e.g. to run a child process. Re-enter with [`Self::resume`]. Like
    /// [`Self::finish`] but keeps the screen.
    pub fn pause(&mut self) -> io::Result<()> {
        self.stage_teardown()?;
        self.terminal.restore()
    }

    /// Re-acquire the terminal after a [`Self::pause`]: re-enter raw mode,
    /// refit the canvas to the current viewport, re-apply the saved
    /// render state and modes, and force a full repaint.
    pub fn resume(&mut self) -> io::Result<()> {
        self.terminal.make_raw()?;
        self.autoresize()?;
        self.canvas.restore();
        self.restore_modes()?;
        self.canvas.invalidate();
        self.canvas.flush()
    }

    fn stage_teardown(&mut self) -> io::Result<()> {
        self.reset_modes()?;
        self.canvas.reset();
        self.canvas.flush()
    }
}

impl Screen<crate::terminal::Stdin, crate::terminal::Stdout> {
    /// Build a screen over the process stdio (`stdin` + `stdout`).
    pub fn stdio() -> io::Result<Self> {
        Self::new(Terminal::stdio())
    }
}

impl Screen<crate::terminal::TtyInput, crate::terminal::TtyOutput> {
    /// Build a screen over the controlling terminal (`/dev/tty`, or
    /// `CONIN$`/`CONOUT$` on Windows), useful when stdio is redirected.
    pub fn open() -> io::Result<Self> {
        Self::new(Terminal::open()?)
    }
}

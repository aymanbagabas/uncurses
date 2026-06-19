//! [`Screen`] — a self-managing terminal application facade.
//!
//! `Screen<I, O>` bundles the three primitives a full-screen terminal
//! program needs into one owned handle:
//!
//! - a [`Terminal`] for the raw-mode lifecycle,
//! - a [`Canvas`] for cell-diffed rendering, and
//! - an [`EventSource`] for decoded input (shared via `Arc<Mutex<_>>` so
//!   it can also back an asynchronous [`EventStream`]).
//!
//! It additionally owns the non-render terminal/input modes (mouse,
//! bracketed paste, focus reporting, in-band resize, the default
//! foreground/background/cursor colors, the window title, the cursor
//! style, and color-scheme update reports) and tracks them so they can be
//! torn down on a shell handoff and re-applied afterwards.
//!
//! Construction enters raw mode and sizes the canvas to the terminal;
//! teardown is explicit — there is **no** `Drop`. Hand the terminal back
//! to the shell with [`Screen::finish`] (consume), [`Screen::pause`]
//! (keep, e.g. to shell out), or [`Screen::suspend`] (pause, then stop
//! the process with `SIGTSTP`); resume a paused/suspended screen with
//! [`Screen::resume`].
//!
//! ```no_run
//! use uncurses::screen::Screen;
//! use uncurses::style::Style;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut screen = Screen::open()?; // raw mode + sized canvas
//! screen.set_alt_screen(true);
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
//! [`EventStream`]: crate::event::EventStream

mod modes;
mod state;

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::canvas::Canvas;
use crate::cell::Cell;
use crate::event::source::Input;
use crate::event::{Event, EventSource};
use crate::layout::Position;
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
    source: Arc<Mutex<EventSource<I>>>,
    modes: state::State,
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

    /// Resize the canvas. See [`Canvas::resize`].
    pub fn resize(&mut self, width: u16, height: u16) {
        self.canvas.resize(width, height);
    }

    /// Force a full repaint on the next render. See [`Canvas::invalidate`].
    pub fn invalidate(&mut self) {
        self.canvas.invalidate();
    }

    /// Drop the cached cursor position so the next render re-emits it.
    /// See [`Canvas::invalidate_cursor`].
    pub fn invalidate_cursor(&mut self) {
        self.canvas.invalidate_cursor();
    }

    /// Insert `content` above the screen. See [`Canvas::insert_above`].
    pub fn insert_above(&mut self, content: &str) {
        self.canvas.insert_above(content);
    }

    /// The canvas width in columns.
    pub fn width(&self) -> u16 {
        self.canvas.width()
    }

    /// The canvas height in rows.
    pub fn height(&self) -> u16 {
        self.canvas.height()
    }

    /// Move the staged cursor. See [`Canvas::set_cursor_position`].
    pub fn set_cursor_position(&mut self, x: u16, y: u16) {
        self.canvas.set_cursor_position(x, y);
    }

    /// Display width of `s` under the canvas's width mode. See
    /// [`Canvas::str_width`].
    pub fn str_width(&self, s: &str) -> u16 {
        self.canvas.str_width(s)
    }

    /// Display width of grapheme `g`. See [`Canvas::grapheme_width`].
    pub fn grapheme_width(&self, g: &str) -> u8 {
        self.canvas.grapheme_width(g)
    }

    // --- Render-coupled mode delegates ----------------------------------

    /// Enter or leave the alternate screen. See [`Canvas::set_alt_screen`].
    pub fn set_alt_screen(&mut self, alt_screen: bool) {
        self.canvas.set_alt_screen(alt_screen);
    }

    /// Show or hide the cursor. See [`Canvas::set_cursor_visible`].
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.canvas.set_cursor_visible(visible);
    }

    /// Enable or disable synchronized updates. See
    /// [`Canvas::set_sync_updates`].
    pub fn set_sync_updates(&mut self, enable: bool) {
        self.canvas.set_sync_updates(enable);
    }

    /// Enable or disable grapheme-cluster width measurement. See
    /// [`Canvas::set_grapheme_clusters`].
    pub fn set_grapheme_clusters(&mut self, enable: bool) {
        self.canvas.set_grapheme_clusters(enable);
    }

    /// Set the per-screen kitty keyboard flags. See
    /// [`Canvas::set_kitty_keyboard_flags`].
    pub fn set_kitty_keyboard_flags(&mut self, flags: crate::ansi::KittyKeyboardFlags) {
        self.canvas.set_kitty_keyboard_flags(flags);
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
        self.source.lock().unwrap().poll(timeout)
    }

    /// Take the next queued event without doing I/O. See
    /// [`EventSource::try_read`].
    pub fn try_read(&mut self) -> Option<Event> {
        self.source.lock().unwrap().try_read()
    }

    /// Block until the next event. See [`EventSource::read`].
    pub fn read(&mut self) -> io::Result<Event> {
        self.source.lock().unwrap().read()
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

#[cfg(unix)]
impl<I, O> Screen<I, O>
where
    I: Input + Copy + std::os::fd::AsFd,
    O: Write + Copy + std::os::fd::AsFd,
{
    /// Build a screen over `terminal`: enter raw mode, size a [`Canvas`]
    /// to the terminal, and start an [`EventSource`] on its input half.
    pub fn new(mut terminal: Terminal<I, O>) -> io::Result<Self> {
        terminal.make_raw()?;
        let canvas = Canvas::from_terminal(&terminal)?;
        let source = EventSource::new(terminal.input())?;
        Ok(Self {
            terminal,
            canvas,
            source: Arc::new(Mutex::new(source)),
            modes: state::State::default(),
        })
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
    /// re-enter raw mode, resize the canvas to the current window size,
    /// re-apply the saved render state and modes, and force a full
    /// repaint.
    pub fn resume(&mut self) -> io::Result<()> {
        self.terminal.make_raw()?;
        let size = self.terminal.window_size().unwrap_or_default();
        self.canvas.resize(size.col, size.row);
        self.canvas.restore();
        self.restore_modes();
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
        self.reset_modes();
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
    /// Build a screen over `terminal`: enter raw mode, size a [`Canvas`]
    /// to the terminal, and start an [`EventSource`] on its input half.
    pub fn new(mut terminal: Terminal<I, O>) -> io::Result<Self> {
        terminal.make_raw()?;
        let canvas = Canvas::from_terminal(&terminal)?;
        let source = EventSource::new(terminal.input())?;
        Ok(Self {
            terminal,
            canvas,
            source: Arc::new(Mutex::new(source)),
            modes: state::State::default(),
        })
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
    /// resize the canvas to the current window size, re-apply the saved
    /// render state and modes, and force a full repaint.
    pub fn resume(&mut self) -> io::Result<()> {
        self.terminal.make_raw()?;
        let size = self.terminal.window_size().unwrap_or_default();
        self.canvas.resize(size.col, size.row);
        self.canvas.restore();
        self.restore_modes();
        self.canvas.invalidate();
        self.canvas.flush()
    }

    fn stage_teardown(&mut self) -> io::Result<()> {
        self.reset_modes();
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

#[cfg(feature = "async")]
impl<I, O> Screen<I, O>
where
    I: Input + 'static,
    O: Write,
{
    /// Open a thread-backed events channel: a [`futures_core::Stream`] of
    /// decoded [`Event`]s sharing this screen's input source. Drive it
    /// with `.next().await`. Synchronous reads ([`Self::read`],
    /// [`Self::poll`], [`Self::try_read`]) and the channel lock the same
    /// source, so the input fd keeps a single owner. See
    /// [`EventStream::from_shared`].
    ///
    /// [`EventStream`]: crate::event::EventStream
    /// [`EventStream::from_shared`]: crate::event::EventStream::from_shared
    pub fn events(&self) -> crate::event::EventStream<I> {
        crate::event::EventStream::from_shared(Arc::clone(&self.source))
    }
}

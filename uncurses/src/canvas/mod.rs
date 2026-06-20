//! Canvas — the orchestrator for rendering + terminal state management.
//!
//! Owns the RenderBuffer (touch-tracked cell grid), the Renderer (output
//! diffing), and the underlying writer.

use std::io::{self, Write};

use crate::ansi::kitty;
use crate::ansi::mode;
use crate::buffer::{Bounded, Surface, SurfaceMut};
use crate::cell::Cell;
use crate::color::Profile;
use crate::renderer::{RenderBuffer, Renderer};
use crate::terminal::{Env, Terminal};

use self::state::State;

mod lifecycle;
mod modes;
mod state;
mod text;

#[cfg(test)]
mod tests;

/// Cell-diff capability flags that control which escape sequences the
/// screen's internal renderer is allowed to emit.
///
/// Re-exported so callers can read or mutate the screen's optimizations
/// without depending on internal renderer modules.
pub use crate::renderer::Optimizations;

/// The main screen abstraction — write cells to a buffer, then render
/// diffs to a terminal.
///
/// The screen owns the underlying writer. Methods that emit output
/// write directly to the owned writer and never flush; flushing is the
/// caller's responsibility — wrap the terminal output in a
/// [`std::io::BufWriter`] (or any other `Write`) and call
/// [`std::io::Write::flush`] on the screen when bytes should hit the
/// wire. Stage everything into a `Vec<u8>` for tests.
///
/// The screen itself implements [`io::Write`] as a passthrough to the
/// owned writer — handy for emitting arbitrary escape sequences around
/// a frame, or flushing pending bytes via [`io::Write::flush`].
///
/// # Managed area
///
/// A `Canvas` manages a rectangular slice of the terminal window and
/// works in two layouts. Either way the screen owns and diffs only the
/// `#` region below; everything else belongs to the terminal.
///
/// **Fullscreen** ([alt screen](Canvas::set_alt_screen) on): the managed
/// area is the *entire* terminal viewport, addressed with absolute cursor
/// moves. Nothing outside it survives.
///
/// ```text
///       +---------------------+  --.
///       |#####################|    |
///       |####             ####|    |
///       |#### managed     ####|    | terminal
///       |#### screen      ####|    | height
///       |#### (= viewport)####|    |
///       |#####################|    |
///       +---------------------+  --'
///       |<---- term width --->|
/// ```
///
/// **Inline** (default, alt screen off): the managed area is the full
/// terminal *width* but only as tall as the application, anchored in the
/// normal buffer with relative cursor moves. Scrollback above and the
/// returning shell prompt below stay in the live terminal, untouched.
///
/// ```text
///       | $ ./app             |  earlier output, left
///       | ...prior output...  |  untouched in scrollback
///       +=====================+  --.
///       |#### managed     ####|    | application
///       |#### screen      ####|    | height
///       +=====================+  --'
///       | $ _                 |  shell prompt returns below
///       |<---- term width --->|
/// ```
///
/// [`resize`](Canvas::resize) sets this area's size: in fullscreen pass
/// the terminal's `(width, height)`; inline, pass the terminal width and
/// the height your application draws.
pub struct Canvas<W: Write> {
    /// The underlying byte sink.
    writer: W,
    /// Touch-tracked cell grid. Holds both the intended frame state and
    /// per-row dirty spans that drive minimal byte emission.
    front_buf: RenderBuffer,
    /// The diff renderer.
    renderer: Renderer,
    /// Scratch byte buffer that every Canvas method (mode changes,
    /// cursor moves, frame diffs, raw [`io::Write`] passthrough)
    /// stages bytes into before [`io::Write::flush`] drains them to
    /// the owned writer.
    buf: Vec<u8>,
    /// Terminal state.
    state: State,
    /// Canvas dimensions.
    width: u16,
    height: u16,
    /// East-Asian Ambiguous policy used when measuring strings: when
    /// `true`, code points whose East-Asian-Width property is
    /// `Ambiguous` are measured as 2 cells instead of 1. Terminals
    /// configured for CJK locales typically want `true`. See
    /// [`crate::text::char_width`].
    eaw_wide: bool,
}

impl<W: Write> Canvas<W> {
    /// Create a new terminal screen with the given writer and initial
    /// `(width, height)` in cells, using a color profile and
    /// optimization set auto-detected from the process environment.
    ///
    /// `size` accepts anything convertible into `(u16, u16)` — a plain
    /// `(width, height)` pair, or a [`Winsize`](crate::terminal::Winsize)
    /// straight from [`get_window_size`](crate::terminal::get_window_size).
    ///
    /// Use [`Canvas::from_env`] to detect from a specific environment,
    /// and the consuming builders
    /// [`with_color_profile`](Canvas::with_color_profile),
    /// [`with_optimizations`](Canvas::with_optimizations), and
    /// [`with_eaw_wide`](Canvas::with_eaw_wide) to override the
    /// detected defaults.
    pub fn new(writer: W, size: impl Into<(u16, u16)>) -> Self {
        Self::from_env(writer, size, &Env::from_process())
    }

    /// Create a new terminal screen, auto-detecting the color profile
    /// and optimization set from `env` instead of the process
    /// environment.
    ///
    /// Useful when the relevant environment isn't this process's own —
    /// for example a remote session whose `TERM` / `COLORTERM` arrive
    /// out of band. `size` accepts anything convertible into
    /// `(u16, u16)`.
    pub fn from_env(writer: W, size: impl Into<(u16, u16)>, env: &Env) -> Self {
        let color_profile = Profile::detect_from(env, true);
        let optimizations = crate::renderer::Optimizations::from_env(env);

        let state = State::default();
        let mut renderer = Renderer::new();
        renderer.set_color_profile(color_profile);
        renderer.set_optimizations(optimizations);
        // Defaults match inline (no alt screen): the surface is anchored
        // wherever the cursor sits and may not be at physical (1,1), so
        // moves must stay relative.
        renderer.set_fullscreen(state.alt_screen);
        renderer.set_relative_cursor(!state.alt_screen);

        let mut screen = Self {
            writer,
            front_buf: RenderBuffer::new(0, 0),
            renderer,
            buf: Vec::with_capacity(4096),
            state,
            width: 0,
            height: 0,
            eaw_wide: false,
        };
        let (w, h) = size.into();
        if w != 0 || h != 0 {
            screen.resize(w, h);
        }
        screen
    }

    /// Build with the East-Asian Ambiguous width policy set (see
    /// [`TextSurface::eaw_wide`](crate::text::TextSurface::eaw_wide)). Consuming builder for use right after
    /// construction.
    pub fn with_eaw_wide(mut self, eaw_wide: bool) -> Self {
        self.eaw_wide = eaw_wide;
        self
    }

    /// Build with an explicit color [`Profile`], overriding the
    /// auto-detected one. Consuming builder; see
    /// [`Canvas::use_color_profile`] to change it at runtime.
    pub fn with_color_profile(mut self, profile: Profile) -> Self {
        self.use_color_profile(profile);
        self
    }

    /// Build with an explicit [`Optimizations`] set, overriding the
    /// auto-detected one. Consuming builder; see
    /// [`Canvas::use_optimizations`] to change it at runtime.
    pub fn with_optimizations(mut self, optimizations: Optimizations) -> Self {
        self.use_optimizations(optimizations);
        self
    }

    /// Switch to a different color [`Profile`] at runtime — for example
    /// to upgrade the profile after confirming richer terminal support.
    /// Affects subsequent frames; call [`Canvas::invalidate`] to
    /// repaint already-rendered content with the new profile.
    pub fn use_color_profile(&mut self, profile: Profile) {
        self.renderer.set_color_profile(profile);
    }

    /// Switch to a different [`Optimizations`] set at runtime — for
    /// example to enable capabilities confirmed by querying the
    /// terminal. Affects subsequent frames.
    pub fn use_optimizations(&mut self, optimizations: Optimizations) {
        self.renderer.set_optimizations(optimizations);
    }

    /// The active [`Profile`] used when emitting cell styles.
    pub fn color_profile(&self) -> Profile {
        self.renderer.color_profile()
    }

    // --- Kitty keyboard --------------------------------------------------

    /// Set the active Kitty keyboard enhancement flags. Emits
    /// `CSI = <flags> ; 1 u` (set-and-replace) targeting the
    /// currently-active screen buffer's top stack frame, and remembers
    /// the desired flag set so it can be re-emitted onto whichever
    /// buffer becomes active afterwards.
    ///
    /// The kitty keyboard stack is per-screen-buffer in the terminal.
    /// Rather than expose that detail, the screen treats its tracked
    /// flag set as the single source of truth and re-applies it on
    /// every alt-screen toggle, on [`Canvas::restore`], and clears it
    /// on [`Canvas::reset`]. Pass [`crate::ansi::KittyKeyboardFlags::NONE`]
    /// (the empty set) to clear every enhancement.
    pub fn set_kitty_keyboard_flags(&mut self, flags: crate::ansi::KittyKeyboardFlags) {
        if self.state.kitty_keyboard != flags {
            kitty::write_set_kitty_keyboard(
                &mut self.buf,
                flags,
                crate::ansi::KittyKeyboardMode::Set,
            )
            .unwrap();
            self.state.kitty_keyboard = flags;
        }
    }

    /// Return the canvas width in cells.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Return the canvas height in cells.
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Whether the alternate screen is currently active.
    pub fn alt_screen(&self) -> bool {
        self.state.alt_screen
    }

    /// Borrow the underlying writer immutably. Useful for inspecting
    /// buffered output in tests / benches when the writer is a
    /// `Vec<u8>` or similar in-memory sink.
    pub fn writer(&self) -> &W {
        &self.writer
    }

    /// Borrow the underlying writer mutably. Lets callers drain or
    /// clear an in-memory sink between frames without dropping the
    /// screen.
    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Return the current cell-diff optimization set.
    pub fn optimizations(&self) -> crate::renderer::Optimizations {
        self.renderer.optimizations()
    }

    /// Set a cell directly.
    pub fn set_cell(&mut self, pos: impl Into<crate::layout::Position>, cell: &Cell) {
        let pos = pos.into();
        self.front_buf.set_cell(pos, cell);
    }

    /// Mutable handle to the cell at `pos`, marking that column as
    /// touched. Returns `None` for out-of-bounds positions.
    ///
    /// Use this when you want to mutate an existing cell in place
    /// (e.g. update its character or style) without paying the
    /// allocate-compare-clone cost of [`Self::set_cell`]. The diff
    /// pipeline filters unchanged cells later via reference equality,
    /// so writing the same value back is cheap.
    ///
    /// Callers must not change [`Cell::width`] through this handle —
    /// width changes require continuation-column accounting that only
    /// [`Self::set_cell`] performs.
    pub fn cell_mut(&mut self, pos: impl Into<crate::layout::Position>) -> Option<&mut Cell> {
        self.front_buf.cell_mut(pos)
    }

    /// Queue a cursor move to `(x, y)` (buffer-relative, origin at
    /// top-left). The move bytes are appended to `buf` and
    /// reach the terminal on the next [`io::Write::flush`].
    ///
    /// No-op when the renderer already reports the cursor at `(x, y)`
    /// **and** that tracked position is known to match the terminal
    /// on both axes. After [`Self::invalidate_cursor`] the next call
    /// always emits a move so the terminal cursor is reasserted.
    pub fn set_cursor_position(&mut self, x: u16, y: u16) {
        let target = crate::layout::Position::new(x, y);
        if self.renderer.cursor_known() && self.renderer.cursor_position() == target {
            return;
        }
        self.renderer
            .move_to(&mut self.buf, &self.front_buf, target.y, target.x)
            .unwrap();
    }

    /// The renderer's last tracked cursor position as a [`crate::layout::Position`].
    pub fn cursor_position(&self) -> crate::layout::Position {
        self.renderer.cursor_position()
    }

    /// Mark the tracked cursor position unknown, so the next
    /// [`Self::set_cursor_position`] always emits a move rather than
    /// short-circuiting on a matching tracked position. Use when the
    /// terminal cursor has been moved by a means the renderer cannot see
    /// (e.g. a raw escape written directly to the screen buffer).
    pub fn invalidate_cursor(&mut self) {
        self.renderer.invalidate_cursor();
    }

    /// Tell the renderer the terminal cursor is now at buffer-relative
    /// `(x, y)`, with both axes known, *without* emitting any move. The
    /// caller must have already placed the terminal cursor there (e.g. via
    /// a raw escape the renderer can't see).
    ///
    /// Prefer this over [`Self::invalidate_cursor`] when the new position
    /// is known: in relative-cursor mode an invalidated cursor can only
    /// re-home its column (`\r`), so the next frame's vertical moves would
    /// be computed from a stale row. Asserting the exact position keeps
    /// those relative moves correct.
    pub fn assume_cursor_at(&mut self, x: u16, y: u16) {
        self.renderer
            .set_cursor_position(crate::layout::Position::new(x, y));
    }

    /// Write the cell-diff sequences (wrapped in cursor-hide and, when
    /// enabled, synchronized-output) for any touched cells directly into
    /// `w`.
    ///
    /// Skips emitting any wrapper bytes when the underlying renderer
    /// reports no work to do, so a no-op render is genuinely zero bytes.
    ///
    /// Only writes — never flushes. Call [`std::io::Write::flush`] on
    /// the writer when the frame should reach the terminal.
    pub fn render(&mut self) {
        if !self.renderer.sync_front(&mut self.front_buf) {
            return;
        }
        self.write_frame();
    }

    /// Render the next frame and flush it to the writer in one call.
    ///
    /// Convenience for [`Canvas::render`] followed by
    /// [`std::io::Write::flush`]: composes the pending frame's diff and
    /// commits it to the terminal. Like [`Canvas::render`], a no-op
    /// frame emits zero bytes.
    pub fn present(&mut self) -> io::Result<()> {
        self.render();
        self.flush()
    }

    /// Stage a single rendered frame into [`Canvas::buf`]:
    /// synchronized-output begin, cursor hide (so the cursor doesn't
    /// dance across cells during the diff), the renderer's cell diff,
    /// cursor show, synchronized-output end. Assumes
    /// [`Renderer::sync_front`] returned true.
    ///
    /// The cursor hide/show wrap is emitted inside the sync-output wrap
    /// so terminals that support DECSET 2026 treat the whole frame as
    /// atomic. The wrap is skipped entirely when the caller has already
    /// hidden the cursor via [`Canvas::set_cursor_visible`].
    ///
    /// Only stages into the buffer, so it is infallible; the bytes reach
    /// the terminal on the next [`Canvas::flush`].
    fn write_frame(&mut self) {
        if self.state.sync_updates {
            mode::Mode::SYNCHRONIZED_OUTPUT.set(&mut self.buf).unwrap();
        }
        if self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.reset(&mut self.buf).unwrap();
        }

        self.renderer.render_back(&mut self.buf).unwrap();

        if self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.set(&mut self.buf).unwrap();
        }
        if self.state.sync_updates {
            mode::Mode::SYNCHRONIZED_OUTPUT
                .reset(&mut self.buf)
                .unwrap();
        }
    }

    /// Resize the screen.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.front_buf.resize(width, height);
        self.renderer.request_clear();
    }

    /// Force a full redraw on next render.
    pub fn invalidate(&mut self) {
        self.renderer.request_clear();
    }
}

#[cfg(unix)]
impl<O: Write + Copy + std::os::fd::AsFd> Canvas<O> {
    /// Build a screen over `terminal`'s output half, sized to the
    /// terminal's current window size and configured from the terminal's
    /// captured [`Env`].
    ///
    /// The terminal is borrowed, not consumed, so it stays available for
    /// the raw-mode lifecycle (`make_raw` / `restore`). The screen drives
    /// the `Copy` output half, leaving the input half free for an
    /// [`EventSource`](crate::event::EventSource). Equivalent to
    /// `Canvas::from_env(terminal.output(), terminal.get_window_size()?, terminal.env())`.
    ///
    /// Fails only if the window size query fails.
    ///
    /// ```no_run
    /// use std::io::Write;
    /// use uncurses::terminal::Terminal;
    /// use uncurses::canvas::Canvas;
    /// use uncurses::event::EventSource;
    ///
    /// # fn main() -> std::io::Result<()> {
    /// let mut term = Terminal::open()?;
    /// let _prev = term.make_raw()?;
    /// let mut screen = Canvas::from_terminal(&term)?;
    /// let mut source = EventSource::new(term.input())?;
    /// // ... draw to `screen`, read from `source` ...
    /// screen.reset();
    /// screen.flush()?;
    /// term.restore()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_terminal<I: std::os::fd::AsFd>(terminal: &Terminal<I, O>) -> io::Result<Self> {
        Ok(Self::from_env(
            terminal.output(),
            terminal.get_window_size()?,
            terminal.env(),
        ))
    }
}

#[cfg(windows)]
impl<O: Write + Copy + std::os::windows::io::AsHandle> Canvas<O> {
    /// Build a screen over `terminal`'s output half, sized to the
    /// terminal's current window size and configured from the terminal's
    /// captured [`Env`].
    ///
    /// The terminal is borrowed, not consumed, so it stays available for
    /// the raw-mode lifecycle (`make_raw` / `restore`). The screen drives
    /// the `Copy` output half, leaving the input half free for an
    /// [`EventSource`](crate::event::EventSource). Equivalent to
    /// `Canvas::from_env(terminal.output(), terminal.get_window_size()?, terminal.env())`.
    ///
    /// Fails only if the window size query fails.
    ///
    /// ```no_run
    /// use std::io::Write;
    /// use uncurses::terminal::Terminal;
    /// use uncurses::canvas::Canvas;
    /// use uncurses::event::EventSource;
    ///
    /// # fn main() -> std::io::Result<()> {
    /// let mut term = Terminal::open()?;
    /// let _prev = term.make_raw()?;
    /// let mut screen = Canvas::from_terminal(&term)?;
    /// let mut source = EventSource::new(term.input())?;
    /// // ... draw to `screen`, read from `source` ...
    /// screen.reset();
    /// screen.flush()?;
    /// term.restore()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_terminal<I: std::os::windows::io::AsHandle>(
        terminal: &Terminal<I, O>,
    ) -> io::Result<Self> {
        Ok(Self::from_env(
            terminal.output(),
            terminal.get_window_size()?,
            terminal.env(),
        ))
    }
}

impl<W: Write> Bounded for Canvas<W> {
    fn bounds(&self) -> crate::layout::Rect {
        self.front_buf.bounds()
    }
}

impl<W: Write> Surface for Canvas<W> {
    fn cell(&self, pos: crate::layout::Position) -> Option<&Cell> {
        self.front_buf.cell(pos)
    }
}

impl<W: Write> SurfaceMut for Canvas<W> {
    fn set_cell(&mut self, pos: crate::layout::Position, cell: &Cell) {
        self.front_buf.set_cell(pos, cell);
    }

    fn cell_mut(&mut self, pos: crate::layout::Position) -> Option<&mut Cell> {
        self.front_buf.cell_mut(pos)
    }

    fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.front_buf.insert_lines(y, n, bounds_bottom, fill);
    }

    fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.front_buf.delete_lines(y, n, bounds_bottom, fill);
    }

    fn insert_cells(
        &mut self,
        pos: crate::layout::Position,
        n: u16,
        bounds_right: u16,
        fill: &Cell,
    ) {
        self.front_buf.insert_cells(pos, n, bounds_right, fill);
    }

    fn delete_cells(
        &mut self,
        pos: crate::layout::Position,
        n: u16,
        bounds_right: u16,
        fill: &Cell,
    ) {
        self.front_buf.delete_cells(pos, n, bounds_right, fill);
    }
}

impl<W: Write> Write for Canvas<W> {
    /// Append raw bytes into `buf`. The bytes do not reach
    /// the underlying writer until [`Write::flush`] is called.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    /// Drain `buf` into the owned writer and flush it.
    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            #[cfg(debug_assertions)]
            crate::trace::tee_output(&self.buf);
            self.writer.write_all(&self.buf)?;
            self.buf.clear();
        }
        self.writer.flush()
    }
}

//! Cell-grid canvas over any [`Write`] sink.
//!
//! A [`Canvas`] is the high-level rendering surface for terminal
//! applications. It owns the caller-facing cell grid, tracks terminal
//! modes that must be restored around shell handoffs, and delegates
//! byte-level diffing to the renderer.
//!
//! ## Buffer model
//!
//! Drawing APIs mutate a touch-tracked `RenderBuffer` owned by the
//! canvas. On [`Canvas::render`], touched spans are copied into the
//! renderer's staging buffer only when the cell value actually differs
//! from the previously staged value. The renderer then compares that
//! staging buffer with its tracked on-screen buffer and updates the
//! tracked buffer as bytes are emitted.
//!
//! ```text
//! set_cell / set_str ─▶ front_buf (the desired frame)
//!                         │ touched spans
//!                         ▼
//! render() ─▶ diff front_buf vs the renderer's tracked screen
//!                         │
//!                         ├─▶ minimal escape bytes ─▶ canvas byte buffer
//!                         ▼
//!             tracked screen updated to match
//!                                  │
//!             flush() / present() ─┴─▶ Write
//! ```
//!
//! ## `render`, `flush`, and `present`
//!
//! [`Canvas::render`] is infallible because it writes only into an
//! internal `Vec<u8>`. It stages mode wrappers, cursor moves, style
//! changes, and cell bytes, but it does not touch the underlying
//! writer. [`std::io::Write::flush`] drains all staged bytes (including
//! raw bytes written through the `Write` implementation) into the owned
//! writer and flushes that writer. [`Canvas::present`] is the
//! convenience boundary: render one frame, then flush it.
//!
//! ## Managed area
//!
//! In inline mode the canvas manages a full-width rectangle anchored at
//! the current terminal cursor and uses relative cursor movement so
//! scrollback above the application stays intact. In alternate-screen
//! mode the managed rectangle is the whole viewport and absolute cursor
//! movement is available.
//!
//! ## Optimizations
//!
//! [`Optimizations`] describe terminal capabilities the renderer may use
//! when two byte sequences are visually equivalent: erase-character,
//! repeat-character, insert/delete cells or lines, scroll regions,
//! absolute column/row addressing, hardware tabs, backspace, and ONLCR
//! newline behavior. Disabling a flag makes the renderer choose more
//! conservative bytes; it should not change the intended cell result.

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
/// canvas renderer may emit.
///
/// # Usage
///
/// Use [`Canvas::optimizations`] to inspect the active set and
/// [`Canvas::use_optimizations`] or [`Canvas::with_optimizations`] to
/// override auto-detection. The type is re-exported here so applications
/// can configure rendering without depending on renderer internals.
pub use crate::renderer::Optimizations;

/// Terminal drawing surface backed by a cell grid and an owned byte sink.
///
/// `Canvas<W>` owns `W`, an in-memory byte buffer, a desired cell grid,
/// and a renderer with tracked terminal state. Drawing methods update
/// cells; terminal-mode methods stage escape bytes; [`Canvas::render`]
/// turns changed cells into escape bytes; [`std::io::Write::flush`]
/// sends staged bytes to `W`.
///
/// # Type parameter
///
/// - `W`: any [`Write`] sink. Use a terminal output handle for live
///   rendering, a [`std::io::BufWriter`] for buffered terminal output,
///   or `Vec<u8>` in tests.
///
/// # I/O model
///
/// `Canvas` implements [`io::Write`], but `write` appends bytes to the
/// canvas staging buffer rather than writing through immediately. This
/// lets raw escape bytes, mode toggles, and rendered frame bytes flush
/// in a single ordered batch.
///
/// # Panics
///
/// Public infallible methods write only into `Vec<u8>` and do not panic
/// for I/O errors. Fallible I/O is reported by [`std::io::Write::flush`]
/// and [`Canvas::present`].
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
    /// Canvas-owned desired cell grid. Touched spans record where the
    /// application wrote since the last sync; the renderer filters them
    /// again against its staging buffer before diffing the terminal.
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
    /// Create a canvas using the process environment for capability
    /// detection.
    ///
    /// # Parameters
    ///
    /// - `writer`: byte sink owned by the canvas.
    /// - `size`: initial `(width, height)` in cells. Accepts a tuple or a
    ///   [`Winsize`](crate::terminal::Winsize).
    ///
    /// # Returns
    ///
    /// A canvas with color profile and renderer optimizations detected
    /// from [`Env::from_process`]. A zero size is allowed; no cells are
    /// allocated until a later [`Canvas::resize`].
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
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

    /// Create a canvas using an explicit environment for capability
    /// detection.
    ///
    /// # Parameters
    ///
    /// - `writer`: byte sink owned by the canvas.
    /// - `size`: initial `(width, height)` in cells. Accepts a tuple or a
    ///   [`Winsize`](crate::terminal::Winsize).
    /// - `env`: environment used for color-profile and optimization
    ///   detection.
    ///
    /// # Returns
    ///
    /// A canvas configured from `env`.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Useful when the relevant environment isn't this process's own —
    /// for example a remote session whose `TERM` / `COLORTERM` arrive
    /// out of band.
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

    /// Set the East-Asian Ambiguous width policy during construction.
    ///
    /// # Parameters
    ///
    /// - `eaw_wide`: when `true`, characters with East-Asian-Width
    ///   `Ambiguous` measure as two cells; when `false`, as one cell.
    ///
    /// # Returns
    ///
    /// The same canvas with the policy updated.
    ///
    /// # Usage notes
    ///
    /// This affects string measurement through
    /// [`TextSurface::eaw_wide`](crate::text::TextSurface::eaw_wide) and
    /// should be chosen before drawing text whose width depends on that
    /// policy.
    pub fn with_eaw_wide(mut self, eaw_wide: bool) -> Self {
        self.eaw_wide = eaw_wide;
        self
    }

    /// Set an explicit color [`Profile`] during construction.
    ///
    /// # Parameters
    ///
    /// - `profile`: color output profile to use for subsequent style
    ///   emission.
    ///
    /// # Returns
    ///
    /// The same canvas with the renderer profile updated.
    ///
    /// # Usage notes
    ///
    /// This overrides auto-detection. Use [`Canvas::use_color_profile`]
    /// to change the profile after construction.
    pub fn with_color_profile(mut self, profile: Profile) -> Self {
        self.use_color_profile(profile);
        self
    }

    /// Set an explicit [`Optimizations`] set during construction.
    ///
    /// # Parameters
    ///
    /// - `optimizations`: capability flags the renderer may rely on.
    ///
    /// # Returns
    ///
    /// The same canvas with renderer optimizations updated.
    ///
    /// # Usage notes
    ///
    /// This overrides auto-detection. Use [`Canvas::use_optimizations`]
    /// to change the set after construction.
    pub fn with_optimizations(mut self, optimizations: Optimizations) -> Self {
        self.use_optimizations(optimizations);
        self
    }

    /// Switch to a different color [`Profile`] at runtime.
    ///
    /// # Parameters
    ///
    /// - `profile`: color output profile used by subsequent style diffs.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Affects future emission only. If already-rendered cells should be
    /// repainted under the new profile, call [`Canvas::invalidate`]
    /// before the next [`Canvas::render`].
    pub fn use_color_profile(&mut self, profile: Profile) {
        self.renderer.set_color_profile(profile);
    }

    /// Switch to a different [`Optimizations`] set at runtime.
    ///
    /// # Parameters
    ///
    /// - `optimizations`: capability flags allowed for subsequent frame
    ///   diffs and cursor movement.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// Existing cell contents are unchanged; only the byte sequences
    /// chosen for later renders are affected.
    pub fn use_optimizations(&mut self, optimizations: Optimizations) {
        self.renderer.set_optimizations(optimizations);
    }

    /// Return the active [`Profile`] used when emitting cell styles.
    ///
    /// # Returns
    ///
    /// The renderer's current color profile.
    pub fn color_profile(&self) -> Profile {
        self.renderer.color_profile()
    }

    // --- Kitty keyboard --------------------------------------------------

    /// Set the active Kitty keyboard enhancement flags.
    ///
    /// # Parameters
    ///
    /// - `flags`: replacement flag set for the terminal's top keyboard
    ///   enhancement stack frame. Pass
    ///   [`crate::ansi::KittyKeyboardFlags::NONE`] to clear all tracked
    ///   enhancements.
    ///
    /// # Behavior
    ///
    /// Stages `CSI = <flags> ; 1 u` for the currently-active screen
    /// buffer and remembers the desired set so it can be re-emitted onto
    /// whichever buffer becomes active afterwards.
    ///
    /// # Panics
    ///
    /// Never panics.
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
    ///
    /// # Returns
    ///
    /// The width most recently passed to [`Canvas::new`],
    /// [`Canvas::from_env`], or [`Canvas::resize`].
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Return the canvas height in cells.
    ///
    /// # Returns
    ///
    /// The height most recently passed to [`Canvas::new`],
    /// [`Canvas::from_env`], or [`Canvas::resize`].
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Return whether the alternate screen is currently active.
    ///
    /// # Returns
    ///
    /// `true` after [`Canvas::set_alt_screen(true)`](Canvas::set_alt_screen)
    /// and `false` after disabling it.
    pub fn alt_screen(&self) -> bool {
        self.state.alt_screen
    }

    /// Borrow the underlying writer immutably.
    ///
    /// # Returns
    ///
    /// Shared access to the owned writer. Pending bytes staged in the
    /// canvas byte buffer are not reflected here until
    /// [`std::io::Write::flush`] succeeds.
    ///
    /// # Usage notes
    ///
    /// Useful for inspecting output when `W` is an in-memory sink such
    /// as `Vec<u8>`.
    pub fn writer(&self) -> &W {
        &self.writer
    }

    /// Borrow the underlying writer mutably.
    ///
    /// # Returns
    ///
    /// Mutable access to the owned writer. Pending bytes staged in the
    /// canvas byte buffer remain pending until
    /// [`std::io::Write::flush`] succeeds.
    ///
    /// # Usage notes
    ///
    /// Lets callers drain or clear an in-memory sink between frames
    /// without dropping the canvas.
    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Return the current cell-diff optimization set.
    ///
    /// # Returns
    ///
    /// The renderer capability flags currently used for cursor planning,
    /// clears, repeats, insert/delete operations, scrolls, and tab
    /// handling.
    pub fn optimizations(&self) -> crate::renderer::Optimizations {
        self.renderer.optimizations()
    }

    /// Set one cell in the desired frame.
    ///
    /// # Parameters
    ///
    /// - `pos`: zero-based canvas coordinate.
    /// - `cell`: cell value to clone into the grid.
    ///
    /// # Behavior
    ///
    /// The write is ignored when `pos` is out of bounds. In-bounds writes
    /// update wide-cell continuation columns through the underlying
    /// buffer and mark the affected columns as touched only when the cell
    /// value changes.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// This stages cell state only; call [`Canvas::render`] or
    /// [`Canvas::present`] to emit terminal bytes.
    pub fn set_cell(&mut self, pos: impl Into<crate::layout::Position>, cell: &Cell) {
        let pos = pos.into();
        self.front_buf.set_cell(pos, cell);
    }

    /// Borrow a cell mutably and mark its occupied columns as touched.
    ///
    /// # Parameters
    ///
    /// - `pos`: zero-based canvas coordinate.
    ///
    /// # Returns
    ///
    /// `Some(&mut Cell)` for an in-bounds cell, or `None` when `pos` is
    /// outside the canvas.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// Use this when you want to mutate an existing cell in place
    /// (e.g. update its character or style) without paying the
    /// allocate-compare-clone cost of [`Self::set_cell`]. The diff
    /// pipeline filters unchanged cells later via value equality,
    /// so writing the same value back is cheap.
    ///
    /// Callers must not change [`Cell::width`] through this handle —
    /// width changes require continuation-column accounting that only
    /// [`Self::set_cell`] performs.
    pub fn cell_mut(&mut self, pos: impl Into<crate::layout::Position>) -> Option<&mut Cell> {
        self.front_buf.cell_mut(pos)
    }

    /// Queue a cursor move to a canvas-relative position.
    ///
    /// # Parameters
    ///
    /// - `x`: zero-based target column.
    /// - `y`: zero-based target row.
    ///
    /// # Behavior
    ///
    /// The renderer plans an optimal move from its tracked cursor state
    /// and appends the bytes to the canvas staging buffer. They reach the
    /// terminal on the next [`io::Write::flush`].
    ///
    /// # Panics
    ///
    /// Never panics.
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

    /// Return the renderer's last tracked cursor position.
    ///
    /// # Returns
    ///
    /// A canvas-relative [`crate::layout::Position`]. The value is the
    /// renderer's model of terminal state; after raw writes that move the
    /// cursor, call [`Canvas::invalidate_cursor`] or
    /// [`Canvas::assume_cursor_at`] to keep the model accurate.
    pub fn cursor_position(&self) -> crate::layout::Position {
        self.renderer.cursor_position()
    }

    /// Mark the tracked cursor position unknown.
    ///
    /// # Behavior
    ///
    /// The next [`Self::set_cursor_position`] or render-time move
    /// reasserts position instead of trusting the cached coordinates.
    ///
    /// # Usage notes
    ///
    /// Use this after raw bytes written through [`io::Write::write`]
    /// move the terminal cursor in a way the renderer cannot observe.
    pub fn invalidate_cursor(&mut self) {
        self.renderer.invalidate_cursor();
    }

    /// Assert the terminal cursor is already at a canvas-relative
    /// position.
    ///
    /// # Parameters
    ///
    /// - `x`: zero-based current column.
    /// - `y`: zero-based current row.
    ///
    /// # Behavior
    ///
    /// Updates renderer bookkeeping without emitting bytes. Both cursor
    /// axes become known and any right-margin phantom state is cleared.
    ///
    /// # Safety of use
    ///
    /// The caller must already have placed the real terminal cursor at
    /// `(x, y)`, for example with a raw escape sequence.
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

    /// Stage the next frame's diff bytes.
    ///
    /// # Behavior
    ///
    /// Copies touched desired cells into the renderer's staging buffer,
    /// filters out values that are unchanged from the previous staged
    /// state, and when work remains appends the renderer's diff bytes to
    /// the canvas byte buffer. The diff may be wrapped in cursor-hide and
    /// synchronized-output mode sequences depending on canvas state.
    ///
    /// No bytes are appended when there is no real cell change and no
    /// forced clear pending.
    ///
    /// # Panics
    ///
    /// Never panics; this method writes only to an in-memory buffer.
    ///
    /// # Usage notes
    ///
    /// This does not flush. Call [`std::io::Write::flush`] to deliver
    /// staged bytes, or use [`Canvas::present`] to render and flush in
    /// one call.
    pub fn render(&mut self) {
        if !self.renderer.sync_front(&mut self.front_buf) {
            return;
        }
        self.write_frame();
    }

    /// Render the next frame and flush staged bytes to the writer.
    ///
    /// # Returns
    ///
    /// The result of [`std::io::Write::flush`] after [`Canvas::render`]
    /// stages any pending frame bytes.
    ///
    /// # Errors
    ///
    /// Returns any error reported while writing staged bytes to the
    /// underlying writer or flushing that writer.
    ///
    /// # Usage notes
    ///
    /// Convenience for [`Canvas::render`] followed by
    /// [`std::io::Write::flush`]. A no-op frame still calls the
    /// underlying writer's `flush`, but stages no new render bytes.
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

    /// Resize the managed canvas area.
    ///
    /// # Parameters
    ///
    /// - `width`: new width in cells.
    /// - `height`: new height in cells.
    ///
    /// # Behavior
    ///
    /// Resizes the desired cell grid, marks the new grid touched through
    /// the buffer resize, and requests a clear/redraw on the next render
    /// so the renderer's tracked terminal state is reconciled with the
    /// new dimensions.
    ///
    /// # Panics
    ///
    /// Never panics.
    ///
    /// # Usage notes
    ///
    /// In alternate-screen mode pass the terminal viewport size. In
    /// inline mode pass the terminal width and the application surface
    /// height.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.front_buf.resize(width, height);
        self.renderer.request_clear();
    }

    /// Force a full redraw on the next render.
    ///
    /// # Behavior
    ///
    /// Requests a clear/update path even if no cells are currently
    /// touched. The next [`Canvas::render`] stages the appropriate clear
    /// for the current layout and repaints the desired cell grid.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn invalidate(&mut self) {
        self.renderer.request_clear();
    }
}

#[cfg(unix)]
impl<O: Write + Copy + std::os::fd::AsFd> Canvas<O> {
    /// Build a canvas over `terminal`'s output half, sized to the
    /// terminal's current window size and configured from the terminal's
    /// captured [`Env`].
    ///
    /// The terminal is borrowed, not consumed, so it stays available for
    /// the raw-mode lifecycle (`make_raw` / `restore`). The screen drives
    /// the `Copy` output half, leaving the input half free for an
    /// [`EventSource`](crate::event::EventSource). Equivalent to
    /// `Canvas::from_env(terminal.output(), terminal.get_window_size()?, terminal.env())`.
    ///
    /// # Parameters
    ///
    /// - `terminal`: terminal whose output handle, window size, and
    ///   captured environment are used.
    ///
    /// # Returns
    ///
    /// A canvas configured as if by
    /// `Canvas::from_env(terminal.output(), terminal.get_window_size()?, terminal.env())`.
    ///
    /// # Errors
    ///
    /// Fails only if querying the terminal window size fails.
    ///
    /// # Usage notes
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
    /// Build a canvas over `terminal`'s output half, sized to the
    /// terminal's current window size and configured from the terminal's
    /// captured [`Env`].
    ///
    /// The terminal is borrowed, not consumed, so it stays available for
    /// the raw-mode lifecycle (`make_raw` / `restore`). The screen drives
    /// the `Copy` output half, leaving the input half free for an
    /// [`EventSource`](crate::event::EventSource). Equivalent to
    /// `Canvas::from_env(terminal.output(), terminal.get_window_size()?, terminal.env())`.
    ///
    /// # Parameters
    ///
    /// - `terminal`: terminal whose output handle, window size, and
    ///   captured environment are used.
    ///
    /// # Returns
    ///
    /// A canvas configured as if by
    /// `Canvas::from_env(terminal.output(), terminal.get_window_size()?, terminal.env())`.
    ///
    /// # Errors
    ///
    /// Fails only if querying the terminal window size fails.
    ///
    /// # Usage notes
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
    /// Append raw bytes to the canvas staging buffer.
    ///
    /// The bytes are ordered with any mode or render bytes already
    /// staged and do not reach the underlying writer until
    /// [`Write::flush`] is called.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    /// Drain staged bytes into the owned writer and flush it.
    ///
    /// If the staging buffer is empty, this still calls
    /// [`Write::flush`] on the owned writer.
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

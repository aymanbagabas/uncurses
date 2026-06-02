//! Screen — the orchestrator for rendering + terminal state management.
//!
//! Owns the RenderBuffer (touch-tracked cell grid), the Renderer (output
//! diffing), and the underlying writer.

use std::io::{self, Write};

use crate::ansi::mode;
use crate::buffer::{Bounded, Surface, SurfaceMut};
use crate::cell::Cell;
use crate::color::Profile;
use crate::renderer::{RenderBuffer, Renderer};
use crate::terminal::Env;

use self::state::State;

mod capabilities;
mod lifecycle;
mod modes;
mod state;
mod text;

#[cfg(test)]
mod tests;

pub use capabilities::{Capabilities, Feature};

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
pub struct Screen<W: Write> {
    /// The underlying byte sink.
    writer: W,
    /// Touch-tracked cell grid. Holds both the intended frame state and
    /// per-row dirty spans that drive minimal byte emission.
    front_buf: RenderBuffer,
    /// The diff renderer.
    renderer: Renderer,
    /// Scratch byte buffer that every Screen method (mode changes,
    /// cursor moves, frame diffs, raw [`io::Write`] passthrough)
    /// stages bytes into before [`io::Write::flush`] drains them to
    /// the owned writer.
    buf: Vec<u8>,
    /// Terminal state.
    state: State,
    /// Screen dimensions.
    width: u16,
    height: u16,
    /// East-Asian Ambiguous policy used when measuring strings: when
    /// `true`, code points whose East-Asian-Width property is
    /// `Ambiguous` are measured as 2 cells instead of 1. Terminals
    /// configured for CJK locales typically want `true`. See
    /// [`crate::text::char_width`].
    eaw_wide: bool,
}

impl<W: Write> Screen<W> {
    /// Create a new terminal screen with the given writer. The screen
    /// starts at 0×0; call [`Screen::with_size`] (builder) or
    /// [`Screen::resize`] to give it a size before rendering.
    ///
    /// The color profile and renderer optimization set are both seeded
    /// from the current process environment via
    /// [`Profile::detect_from`] and
    /// [`crate::renderer::Optimizations::from_env`]. Override the
    /// environment with [`Screen::with_environment`], or set either
    /// piece directly with [`Screen::with_color_profile`] /
    /// [`Screen::with_optimizations`] when the auto-detected value
    /// isn't appropriate (e.g. tests that write to a `Vec<u8>`).
    pub fn new(writer: W) -> Self {
        let state = State::default();
        let mut renderer = Renderer::new();
        let env = Env::from_process();
        renderer.set_color_profile(Profile::detect_from(&env, true));
        renderer.set_optimizations(crate::renderer::Optimizations::from_env(&env));
        // Defaults match inline (no alt screen): the surface is anchored
        // wherever the cursor sits and may not be at physical (1,1), so
        // moves must stay relative.
        renderer.set_fullscreen(state.alt_screen);
        renderer.set_relative_cursor(!state.alt_screen);

        Self {
            writer,
            front_buf: RenderBuffer::new(0, 0),
            renderer,
            buf: Vec::with_capacity(4096),
            state,
            width: 0,
            height: 0,
            eaw_wide: false,
        }
    }

    /// Builder-style sizing: resize the screen to `width` × `height`.
    pub fn with_size(mut self, width: u16, height: u16) -> Self {
        self.resize(width, height);
        self
    }

    /// Builder-style setter for the East-Asian Ambiguous policy used
    /// when measuring strings (see [`Screen::eaw_wide`]).
    pub fn with_eaw_wide(mut self, eaw_wide: bool) -> Self {
        self.eaw_wide = eaw_wide;
        self
    }

    /// Builder-style override for the [`Profile`] used when
    /// emitting cell styles. Profiles narrower than the cell's color
    /// downgrade the color (e.g. truecolor → ANSI 256) on the next
    /// render. The renderer memoizes conversions internally.
    pub fn with_color_profile(mut self, profile: Profile) -> Self {
        self.renderer.set_color_profile(profile);
        self
    }

    /// Builder-style override for the environment used to detect the
    /// color profile and the renderer's optimization set. Recomputes
    /// the profile via [`Profile::detect_from`] with
    /// `is_tty = true` — the screen always assumes its writer is a
    /// terminal; pass [`Profile::Disabled`] via
    /// [`Screen::with_color_profile`] for non-TTY sinks. Recomputes
    /// the optimization set via
    /// [`crate::renderer::Optimizations::from_env`]; pass a custom set
    /// via [`Screen::with_optimizations`] afterward to override.
    pub fn with_environment(mut self, env: &Env) -> Self {
        self.renderer
            .set_color_profile(Profile::detect_from(env, true));
        self.renderer
            .set_optimizations(crate::renderer::Optimizations::from_env(env));
        self
    }

    /// Builder-style setter for the renderer's cell-diff capability
    /// set. Pass a narrower set when the target terminal lacks one of
    /// the optimizations (e.g. no `ECH`, no relative cursor moves).
    pub fn with_optimizations(mut self, opts: crate::renderer::Optimizations) -> Self {
        self.renderer.set_optimizations(opts);
        self
    }

    /// The active [`Profile`] used when emitting cell styles.
    pub fn color_profile(&self) -> Profile {
        self.renderer.color_profile()
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
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

    /// Return the current cell-diff capability set.
    pub fn optimizations(&self) -> crate::renderer::Optimizations {
        self.renderer.optimizations()
    }

    /// Set a cell directly.
    pub fn set_cell(&mut self, pos: impl Into<crate::Position>, cell: &Cell) {
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
    pub fn cell_mut(&mut self, pos: impl Into<crate::Position>) -> Option<&mut Cell> {
        self.front_buf.cell_mut(pos)
    }

    /// Queue a cursor move to `(x, y)` (buffer-relative, origin at
    /// top-left). The move bytes are appended to [`Screen::buf`] and
    /// reach the terminal on the next [`io::Write::flush`].
    ///
    /// No-op when the renderer already reports the cursor at `(x, y)`.
    pub fn set_cursor_position(&mut self, x: u16, y: u16) -> io::Result<()> {
        let target = crate::Position::new(x, y);
        if self.renderer.cursor_position() == target {
            return Ok(());
        }
        self.renderer
            .move_to(&mut self.buf, &self.front_buf, target.y, target.x)
    }

    /// The renderer's last tracked cursor position as a [`crate::Position`].
    pub fn cursor_position(&self) -> crate::Position {
        self.renderer.cursor_position()
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
    pub fn render(&mut self) -> io::Result<()> {
        if !self.renderer.sync_front(&mut self.front_buf) {
            return Ok(());
        }

        self.write_frame()
    }

    /// Stage a single rendered frame into [`Screen::buf`]:
    /// synchronized-output begin, cursor hide (so the cursor doesn't
    /// dance across cells during the diff), the renderer's cell diff,
    /// cursor show, synchronized-output end. Assumes
    /// [`Renderer::sync_front`] returned true.
    ///
    /// The cursor hide/show wrap is emitted inside the sync-output wrap
    /// so terminals that support DECSET 2026 treat the whole frame as
    /// atomic. The wrap is skipped entirely when the caller has already
    /// hidden the cursor via [`Screen::set_cursor_visible`].
    fn write_frame(&mut self) -> io::Result<()> {
        if self.state.sync_updates {
            mode::Mode::SYNCHRONIZED_OUTPUT.set(&mut self.buf)?;
        }
        if self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.reset(&mut self.buf)?;
        }

        self.renderer.render_back(&mut self.buf)?;

        if self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.set(&mut self.buf)?;
        }
        if self.state.sync_updates {
            mode::Mode::SYNCHRONIZED_OUTPUT.reset(&mut self.buf)?;
        }
        Ok(())
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

    /// Mark the renderer's tracked cursor position as no longer
    /// matching the terminal. The next cursor move reasserts position
    /// via the planner (CR + relative motions).
    ///
    /// Use this after emitting bytes that move the terminal cursor
    /// outside the renderer's bookkeeping (e.g. raw escape sequences
    /// or out-of-band protocol payloads) so the next render still
    /// lands the cursor where it belongs.
    pub fn invalidate_cursor(&mut self) {
        self.renderer.invalidate_cursor();
    }
}

impl<W: Write> Bounded for Screen<W> {
    fn bounds(&self) -> crate::Rect {
        self.front_buf.bounds()
    }
}

impl<W: Write> Surface for Screen<W> {
    fn cell(&self, pos: crate::Position) -> Option<&Cell> {
        self.front_buf.cell(pos)
    }
}

impl<W: Write> SurfaceMut for Screen<W> {
    fn set_cell(&mut self, pos: crate::Position, cell: &Cell) {
        self.front_buf.set_cell(pos, cell);
    }

    fn cell_mut(&mut self, pos: crate::Position) -> Option<&mut Cell> {
        self.front_buf.cell_mut(pos)
    }

    fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.front_buf.insert_lines(y, n, bounds_bottom, fill);
    }

    fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.front_buf.delete_lines(y, n, bounds_bottom, fill);
    }

    fn insert_cells(&mut self, pos: crate::Position, n: u16, bounds_right: u16, fill: &Cell) {
        self.front_buf.insert_cells(pos, n, bounds_right, fill);
    }

    fn delete_cells(&mut self, pos: crate::Position, n: u16, bounds_right: u16, fill: &Cell) {
        self.front_buf.delete_cells(pos, n, bounds_right, fill);
    }
}

impl<W: Write> Write for Screen<W> {
    /// Append raw bytes into [`Screen::buf`]. The bytes do not reach
    /// the underlying writer until [`Write::flush`] is called.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    /// Drain [`Screen::buf`] into the owned writer and flush it.
    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            self.writer.write_all(&self.buf)?;
            self.buf.clear();
        }
        self.writer.flush()
    }
}

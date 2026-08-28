//! [`Screen`] — the cell-diff renderer you draw into.
//!
//! `Screen<W>` owns a desired cell grid, a diff renderer, and a writer. You
//! paint cells into it and call [`render`](Screen::render); it works out the
//! minimal escape sequence that turns what the terminal is showing into what
//! you asked for, and writes it.
//!
//! That is the whole job. `Screen` reads no input, tracks no capabilities,
//! and manages no session — [`Program`](crate::program::Program) does all of
//! that and owns a `Screen` to render with. A `Screen` on its own is enough
//! for output-only programs, tests, and offscreen rendering, since `W` is any
//! [`Write`]: a terminal handle, a `Vec<u8>`, a file.
//!
//! ```
//! use uncurses::screen::Screen;
//! use uncurses::style::Style;
//! use uncurses::text::TextSurface;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut screen = Screen::new(Vec::new(), (20, 3));
//! screen.set_str((0, 0), "hello", Style::default());
//! screen.render()?;
//! assert!(!screen.writer().is_empty());
//! # Ok(())
//! # }
//! ```
//!
//! # Render properties
//!
//! A `Screen` holds only the state that changes how a frame is drawn:
//!
//! * [`fullscreen`](Screen::fullscreen) — whether the managed area is the
//!   whole viewport (the alternate screen buffer, addressed with absolute
//!   moves) or a band in the normal buffer (the default, addressed
//!   relatively so scrollback above and the shell prompt below survive).
//! * Cursor visibility and the declarative resting
//!   [position](Screen::set_cursor_position).
//! * [Synchronized output](Screen::set_synchronized_output), grapheme-cluster
//!   [width mode](Screen::set_grapheme_clusters), the
//!   [color profile](Screen::set_color_profile), and the renderer
//!   [optimizations](Screen::set_optimizations).
//!
//! Every one of these is a plain setter: it cannot fail and it writes nothing.
//! A `Screen` never *persists* a terminal mode: the only modes it emits are
//! the synchronized-output and cursor-visibility markers it wraps a single
//! frame in, and it closes both before [`render`](Screen::render) returns.
//! Leaving a mode on is
//! [`Program`](crate::program::Program)'s job, and it pushes the render
//! consequence down here with the matching setter — so
//! [`Program::enter_alt_screen`](crate::program::Program::enter_alt_screen)
//! emits DECSET 1049 and calls [`set_fullscreen`](Screen::set_fullscreen), and
//! [`Program::hide_cursor`](crate::program::Program::hide_cursor) emits DECTCEM
//! and calls [`set_cursor_visible`](Screen::set_cursor_visible). Drive a
//! `Screen` yourself and you own both halves.
//!
//! ```text
//!  Inline (default): the surface lives in the normal buffer, only as
//!  many rows as you draw; scrollback and the shell prompt stay intact.
//!
//!    $ earlier shell output
//!    $ ... scrollback ...
//!    ┌─────────────────────────┐
//!    │ managed surface         │  <- only the rows you draw, full width
//!    └─────────────────────────┘
//!    $ shell prompt resumes
//!
//!  Fullscreen: the whole viewport is the surface, addressed with
//!  absolute moves, and restored on exit.
//!
//!    ┌─────────────────────────────┐
//!    │                             │
//!    │  the whole terminal         │
//!    │  viewport is the surface    │
//!    │                             │
//!    └─────────────────────────────┘
//! ```

#[cfg(test)]
mod tests;

/// Cell-diff capability flags controlling which optimized escape
/// sequences the screen's renderer may emit. Re-exported from the
/// renderer so applications can configure rendering with
/// [`Screen::set_optimizations`] without depending on renderer internals.
pub use crate::renderer::Optimizations;

use std::io::{self, Write};

use crate::ansi::mode;
use crate::buffer::{Bounded, Surface, SurfaceMut};
use crate::cell::Cell;
use crate::layout::{Position, Rect, Size};
use crate::renderer::{RenderBuffer, Renderer};
use crate::text::{TextSurface, WidthMode};

/// A cell-diff renderer over a writer. See the [module documentation](self).
///
/// `Screen` is [`Send`] and [`Sync`] whenever its writer is, so it can be
/// moved onto another thread or held across an `.await` point.
pub struct Screen<W: Write> {
    /// Where rendered bytes go.
    writer: W,
    /// Caller-facing desired cell grid. Touched spans record where the
    /// application wrote since the last sync; the renderer filters them
    /// again against its staging buffer before diffing the terminal.
    front_buf: RenderBuffer,
    /// The diff renderer holding the tracked on-screen buffer, cursor model,
    /// color profile, and optimizations.
    renderer: Renderer,
    /// Scratch byte buffer that drawing and property methods stage escape
    /// bytes into before [`io::Write::flush`] drains them to the writer.
    out_buf: Vec<u8>,
    /// Managed area width in cells.
    width: u16,
    /// Managed area height in cells.
    height: u16,
    /// East-Asian Ambiguous width policy used when measuring strings: when
    /// `true`, code points whose East-Asian-Width property is `Ambiguous`
    /// are measured as 2 cells instead of 1. See [`crate::text::char_width`].
    eaw_wide: bool,
    /// Whether the managed area is the whole viewport or an inline band.
    fullscreen: bool,
    /// Cursor visibility (DECTCEM). Render-coupled: a frame hides a *visible*
    /// cursor around the cell diff, and bracketing a cursor the caller
    /// deliberately hid would turn it back on.
    cursor_visible: bool,
    /// Synchronized updates: when `true`, each non-empty frame is wrapped in
    /// synchronized-output begin/end sequences.
    sync_updates: bool,
    /// Unicode core / grapheme cluster mode (DEC 2027). When `true`, width is
    /// calculated per grapheme cluster (UTS-29 + emoji rules); when `false`,
    /// per code point (wcwidth-style).
    grapheme_clusters: bool,
    /// Declarative resting position for the cursor, applied at the end of
    /// every [`render`](Screen::render) via
    /// [`set_cursor_position`](Screen::set_cursor_position). Sticky: it
    /// persists across frames and is re-applied each render (a no-op when the
    /// cursor is already there) until changed or cleared. `None` means no
    /// declarative resting position, so the cursor is left wherever the cell
    /// diff ended.
    desired_cursor: Option<Position>,
}

impl<W: Write> Screen<W> {
    // --- Construction ---------------------------------------------------

    /// Build a screen rendering into `writer`, with a managed area of `size`.
    ///
    /// Nothing is written and no terminal state is touched. The color profile
    /// defaults to [`Profile::Ansi`](crate::color::Profile::Ansi) and the
    /// optimizations to [`Optimizations::default`]; set them with
    /// [`set_color_profile`](Self::set_color_profile) and
    /// [`set_optimizations`](Self::set_optimizations), or let
    /// [`Program`](crate::program::Program) detect both from the environment.
    pub fn new(writer: W, size: impl Into<Size>) -> Self {
        let mut renderer = Renderer::new();
        // Defaults match inline: the surface is anchored wherever the cursor
        // sits, so moves stay relative.
        renderer.set_fullscreen(false);
        renderer.set_relative_cursor(true);
        let mut screen = Self {
            writer,
            front_buf: RenderBuffer::new(0, 0),
            renderer,
            out_buf: Vec::with_capacity(4096),
            width: 0,
            height: 0,
            eaw_wide: false,
            fullscreen: false,
            cursor_visible: true,
            sync_updates: false,
            grapheme_clusters: false,
            desired_cursor: None,
        };
        let size = size.into();
        if size.width != 0 || size.height != 0 {
            screen.resize(size);
        }
        screen
    }

    /// Borrow the writer frames are rendered into.
    pub fn writer(&self) -> &W {
        &self.writer
    }

    /// Borrow the writer mutably.
    ///
    /// Bytes written straight to the writer bypass the staging buffer, so they
    /// can land *before* escapes already staged by drawing or property
    /// methods. Prefer writing through the screen itself (it implements
    /// [`Write`]), which keeps everything in order.
    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Consume the screen and return its writer, discarding anything still
    /// staged and unflushed.
    pub fn into_writer(self) -> W {
        self.writer
    }

    // --- Drawing -------------------------------------------------------

    /// Write `cell` at `pos` in the desired frame.
    pub fn set_cell(&mut self, pos: impl Into<Position>, cell: &Cell) {
        self.front_buf.set_cell(pos.into(), cell);
    }

    /// Borrow the cell at `pos` mutably, marking its columns touched.
    pub fn cell_mut(&mut self, pos: impl Into<Position>) -> Option<&mut Cell> {
        self.front_buf.cell_mut(pos.into())
    }

    /// The managed area size in cells.
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    /// Diff the staged frame against the tracked terminal, stage the
    /// minimal escape bytes, and flush them to the writer.
    ///
    /// When a declarative cursor rest position has been staged with
    /// [`set_cursor_position`](Self::set_cursor_position), the cursor is moved
    /// there at the end of the frame, inside the same hide/synchronized-output
    /// bracket as the cell diff, so it lands atomically and without flicker.
    pub fn render(&mut self) -> io::Result<()> {
        let changed = self.renderer.sync_front(&mut self.front_buf);
        if changed || self.cursor_move_pending() {
            self.write_frame();
        }
        self.flush()
    }

    /// Force a full redraw on the next [`render`](Self::render).
    pub fn invalidate(&mut self) {
        self.renderer.request_clear();
    }

    /// Resize the managed area. In fullscreen pass the terminal viewport
    /// size; inline, the terminal width and the application surface height.
    ///
    /// Resizing discards the tracked terminal contents, so the next
    /// [`render`](Self::render) is a full repaint. Passing the current size is
    /// a no-op for that reason: [`Program::autoresize`](crate::program::Program::autoresize)
    /// runs on every resize report, and terminals send those for changes that
    /// leave the cell grid alone (a font size change that keeps rows and
    /// columns, a window move on some terminals), so repainting on them would
    /// be flicker with nothing behind it. Use [`invalidate`](Self::invalidate)
    /// to force a repaint on purpose.
    pub fn resize(&mut self, size: impl Into<Size>) {
        let size = size.into();
        if self.width == size.width && self.height == size.height {
            return;
        }
        self.width = size.width;
        self.height = size.height;
        self.front_buf.resize(size.width, size.height);
        self.renderer.request_clear();
    }

    /// Insert `content` into the scrollback above the managed area and flush
    /// it to the writer. Inline this pushes the lines into the terminal's
    /// scrollback; in fullscreen they go into the alternate screen's hidden
    /// scrollback. The managed area is preserved in place, so no redraw is
    /// needed and a following [`render`](Self::render) sees no change. An
    /// empty string is a no-op.
    ///
    /// # Errors
    ///
    /// Returns any error from flushing the inserted lines to the writer.
    pub fn insert_above(&mut self, content: &str) -> io::Result<()> {
        if content.is_empty() {
            return Ok(());
        }

        let width = self.width;
        let height = self.height;
        let y = self.renderer.cursor_position().y;

        self.out_buf.write_all(b"\r").unwrap();
        let down = height.saturating_sub(y).saturating_sub(1);
        if down > 0 {
            crate::ansi::cursor::write_cud(&mut self.out_buf, down).unwrap();
        }

        let lines: Vec<&str> = content.split('\n').collect();
        let mut offset: u16 = lines.len() as u16;
        let width_mode = self.width_mode();
        for line in &lines {
            let lw =
                crate::ansi::text::string_width(line.as_bytes(), width_mode, self.eaw_wide) as u16;
            if let Some(n) = lw.checked_div(width) {
                offset = offset.saturating_add(n);
            }
        }

        for _ in 0..offset {
            self.out_buf.write_all(b"\n").unwrap();
        }

        let up = offset.saturating_add(height).saturating_sub(1);
        if up > 0 {
            crate::ansi::cursor::write_cuu(&mut self.out_buf, up).unwrap();
        }
        crate::ansi::screen::write_insert_lines(&mut self.out_buf, offset).unwrap();
        for line in &lines {
            self.out_buf.write_all(line.as_bytes()).unwrap();
            self.out_buf
                .write_all(crate::ansi::screen::ERASE_LINE_RIGHT)
                .unwrap();
            self.out_buf.write_all(b"\r\n").unwrap();
        }

        self.renderer.set_cursor_position(Position { y: 0, x: 0 });
        self.flush()
    }

    // --- Cursor position ------------------------------------------------

    /// Immediately move the terminal cursor to a buffer-relative position and
    /// flush.
    ///
    /// The target is normalized against the managed area first: a column at or
    /// past the width wraps into the following row, and the row is clamped to
    /// the last row. The move is a no-op once the cursor already sits at that
    /// normalized position, so asking for a column one past the last is a
    /// request to wrap and is honored as one, even when the renderer is
    /// already tracking the cursor there with its wrap pending.
    ///
    /// This is imperative: the move is emitted and flushed now, independent of
    /// [`render`](Self::render). It does **not** affect the declarative resting
    /// position staged with [`set_cursor_position`](Self::set_cursor_position);
    /// a subsequent `render` will snap the cursor back to that sticky position
    /// if one is set. To change where frames leave the cursor, use
    /// `set_cursor_position` instead.
    pub fn move_cursor_to(&mut self, pos: impl Into<Position>) -> io::Result<()> {
        self.stage_move_cursor_to(pos.into());
        self.flush()
    }

    /// Immediately move the terminal cursor relative to the
    /// [tracked cursor](Self::tracked_cursor) and flush.
    ///
    /// Convenience over [`move_cursor_to`](Self::move_cursor_to): the target
    /// is the tracked cursor offset by `(dx, dy)`, saturating at the buffer
    /// origin and then normalized the same way, so a column past the width
    /// wraps into the following row and the row is clamped to the surface. An
    /// unknown tracked cursor is treated as the origin.
    pub fn move_cursor_by(&mut self, dx: i16, dy: i16) -> io::Result<()> {
        let cur = self.tracked_cursor().unwrap_or(Position::ORIGIN);
        let x = cur.x.saturating_add_signed(dx);
        let y = cur.y.saturating_add_signed(dy);
        self.move_cursor_to((x, y))
    }

    /// Stage a declarative resting position for the cursor, applied at the end
    /// of every [`render`](Self::render).
    ///
    /// This is the cursor analogue of [`set_cell`](Self::set_cell): it stages
    /// intent rather than emitting now. `render` leaves the terminal cursor at
    /// the buffer-relative `pos` after each frame's cell diff. Call
    /// [`clear_cursor_position`](Self::clear_cursor_position) to stop steering
    /// it and leave the cursor wherever the diff ended.
    ///
    /// The position is **sticky** — it persists across frames and is re-applied
    /// on every `render` (cheaply, as a no-op when the cursor is already there)
    /// until you change or clear it. An app whose cursor follows content
    /// (e.g. a text field) should call this each time that content moves.
    ///
    /// Cursor visibility is orthogonal: this never shows or hides the cursor.
    /// Use [`set_cursor_visible`](Self::set_cursor_visible) for that (or
    /// [`Program::show_cursor`](crate::program::Program::show_cursor) /
    /// [`hide_cursor`](crate::program::Program::hide_cursor), which also emit
    /// DECTCEM). A position outside the managed area is clamped to its edges.
    ///
    /// The argument is anything that converts into a [`Position`], so a bare
    /// `(x, y)` works:
    ///
    /// ```
    /// # fn main() -> std::io::Result<()> {
    /// let mut screen = uncurses::screen::Screen::new(Vec::new(), (20, 3));
    /// screen.set_cursor_position((4, 0)); // stage
    /// screen.clear_cursor_position();     // stop steering it
    /// # Ok(())
    /// # }
    /// ```
    pub fn set_cursor_position(&mut self, pos: impl Into<Position>) {
        self.desired_cursor = Some(pos.into());
    }

    /// Clear the staged cursor [resting position](Self::set_cursor_position),
    /// leaving the cursor wherever each frame's cell diff ends.
    pub fn clear_cursor_position(&mut self) {
        self.desired_cursor = None;
    }

    /// The renderer's tracked cursor: the buffer-relative cell where the
    /// renderer believes the terminal cursor currently sits, or `None` when
    /// that position is unknown (initially, after a screen reset, or after
    /// [`invalidate_tracked_cursor`](Self::invalidate_tracked_cursor)). This
    /// is bookkeeping, not a live cursor-position query.
    pub fn tracked_cursor(&self) -> Option<Position> {
        self.renderer
            .cursor_known()
            .then(|| self.renderer.cursor_position())
    }

    /// Mark the tracked cursor position unknown, so the next staged move
    /// always emits rather than short-circuiting on a matching tracked
    /// position. Use after moving the terminal cursor by a means the
    /// renderer cannot see (e.g. a raw escape written directly).
    pub fn invalidate_tracked_cursor(&mut self) {
        self.renderer.invalidate_cursor();
    }

    /// Set the tracked cursor to buffer-relative `pos`, with both axes
    /// known, *without* emitting any move. This only updates the renderer's
    /// belief; the caller must have already placed the terminal cursor there
    /// (e.g. with a raw escape the renderer cannot see). For an actual cursor
    /// move use [`move_cursor_to`](Self::move_cursor_to).
    pub fn set_tracked_cursor(&mut self, pos: impl Into<Position>) {
        self.renderer.set_cursor_position(pos.into());
    }

    /// Clamp a buffer-relative position to the managed area's edges.
    fn clamp_to_surface(&self, pos: Position) -> Position {
        Position {
            x: pos.x.min(self.width.saturating_sub(1)),
            y: pos.y.min(self.height.saturating_sub(1)),
        }
    }

    /// Whether a declarative cursor rest position is staged and the renderer's
    /// tracked cursor isn't already there, so [`render`](Self::render) must
    /// emit a move even when no cells changed.
    fn cursor_move_pending(&self) -> bool {
        match self.desired_cursor {
            Some(pos) => {
                let pos = self.clamp_to_surface(pos);
                !self.renderer.cursor_known() || self.renderer.cursor_position() != pos
            }
            None => false,
        }
    }

    // --- Render properties ----------------------------------------------

    /// Set whether the managed area is the whole viewport.
    ///
    /// `true` means the managed area covers the whole terminal and is
    /// addressed with absolute moves — what you want on the alternate screen
    /// buffer. `false` (the default) makes it a band in the normal buffer, as
    /// tall as you draw and addressed with relative moves, leaving the
    /// scrollback above and the shell prompt below intact.
    ///
    /// This only sets state — it emits nothing. Switching screen buffers is a
    /// terminal mode (DECSET/DECRST 1049) and belongs to whoever owns the
    /// terminal: [`Program::enter_alt_screen`] and
    /// [`Program::exit_alt_screen`] emit it and set this for you. Driving a
    /// bare `Screen`, emit the mode yourself and keep this in step, or the
    /// renderer will address the wrong buffer.
    ///
    /// An actual change discards the tracked contents, so the next
    /// [`render`](Self::render) is a full repaint.
    ///
    /// [`Program::enter_alt_screen`]: crate::program::Program::enter_alt_screen
    /// [`Program::exit_alt_screen`]: crate::program::Program::exit_alt_screen
    pub fn set_fullscreen(&mut self, fullscreen: bool) {
        if self.fullscreen == fullscreen {
            return;
        }
        self.fullscreen = fullscreen;
        self.renderer.set_fullscreen(fullscreen);
        self.renderer.set_relative_cursor(!fullscreen);
        if fullscreen {
            self.renderer.save_cursor();
        } else {
            self.renderer.restore_cursor();
        }
        // Either direction swaps which screen buffer the managed area lives
        // on, and the other buffer holds something this renderer never wrote.
        // Diffing against the record from the buffer being left would skip
        // every cell the two happen to agree on, so discard it and repaint.
        self.renderer.request_clear();
    }

    /// Whether the managed area is the whole viewport rather than a band in
    /// the normal buffer. See [`set_fullscreen`](Self::set_fullscreen).
    pub fn fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// Record whether the terminal cursor is visible.
    ///
    /// This only sets state — it emits nothing. DECTCEM is a terminal mode
    /// and belongs to whoever owns the terminal:
    /// [`Program::show_cursor`](crate::program::Program::show_cursor) and
    /// [`Program::hide_cursor`](crate::program::Program::hide_cursor) emit it
    /// and set this for you.
    ///
    /// The renderer needs to know only so it can bracket a frame correctly: a
    /// *visible* cursor is hidden around the cell diff so it does not dance
    /// across cells as the renderer repositions it, and shown again after. If
    /// this said `true` while the cursor was actually hidden, that closing
    /// show would turn it back on.
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor_visible = visible;
    }

    /// Whether the terminal cursor is recorded as visible. See
    /// [`set_cursor_visible`](Self::set_cursor_visible).
    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// Set whether text is measured per extended grapheme cluster (UTS-29
    /// plus emoji presentation rules) rather than per code point
    /// (wcwidth-style). Affects [`set_str`](crate::text::TextSurface::set_str)
    /// and [`insert_above`](Self::insert_above).
    ///
    /// This only sets state — it emits nothing. Unicode core (DECSET 2027) is
    /// a terminal mode and belongs to whoever owns the terminal:
    /// [`Program::enable_grapheme_clusters`] and
    /// [`Program::disable_grapheme_clusters`] emit it and set this for you.
    /// Measuring differently from the terminal misplaces every cell after the
    /// first cluster on a line, so the two must agree.
    ///
    /// Changing the mode discards the tracked terminal contents, so the next
    /// [`render`](Self::render) is a full repaint: what is already on screen
    /// was measured the other way. Setting the current value is a no-op.
    ///
    /// The repaint clears the screen, but buffered cells keep the width they
    /// were measured with, so re-write any text whose measurement changes.
    /// Applications that redraw their content each frame get that for free.
    ///
    /// [`Program::enable_grapheme_clusters`]: crate::program::Program::enable_grapheme_clusters
    /// [`Program::disable_grapheme_clusters`]: crate::program::Program::disable_grapheme_clusters
    pub fn set_grapheme_clusters(&mut self, enabled: bool) {
        if self.grapheme_clusters == enabled {
            return;
        }
        self.grapheme_clusters = enabled;
        // Whatever is on screen was measured under the old model, so the
        // tracked terminal contents no longer describe it. Diffing against
        // that record would leave the two disagreeing about which column
        // holds what, so discard it and repaint.
        self.invalidate();
    }

    /// Whether text is measured per extended grapheme cluster. See
    /// [`set_grapheme_clusters`](Self::set_grapheme_clusters).
    pub fn grapheme_clusters(&self) -> bool {
        self.grapheme_clusters
    }

    /// Enable or disable synchronized-output frame wrapping.
    ///
    /// When enabled, each non-empty [`render`](Self::render) is wrapped in
    /// begin/end synchronized-output sequences (DEC mode 2026) so terminals
    /// that support it present the frame atomically, with no mid-frame
    /// repaint. Terminals that don't support 2026 ignore the markers.
    ///
    /// This is your switch to flip: uncurses does not second-guess it against
    /// detected capabilities. [`Program`](crate::program::Program) enables it
    /// automatically when the terminal reports 2026 support, which happens once
    /// the caller has asked and read the reply, and you can override that here
    /// at any time.
    ///
    /// Enabling it also changes how the cursor is handled per frame. With sync
    /// off, a visible cursor is hidden around the cell diff so it doesn't dance
    /// across cells as the renderer repositions it. With sync on, the frame is
    /// presented in one step, so that hide/show pair is dropped: it is
    /// redundant, and toggling the cursor every frame resets its blink phase,
    /// which reads as flicker.
    ///
    /// This only sets state; the markers are emitted on the next `render`.
    /// Scroll detection is gated on this, so enabling it is what lets
    /// [`set_scroll_optimize`](Self::set_scroll_optimize) take effect; that
    /// setting still has to be on, and the screen still has to be
    /// [fullscreen](Self::set_fullscreen). A terminal that advertises DEC
    /// 2026 but does not honour it therefore gets both the markers and the
    /// scroll plans that rely on them, and a scroll's corrective repaint may
    /// be visible. Disable scroll optimization on such a terminal.
    ///
    pub fn set_synchronized_output(&mut self, enabled: bool) {
        self.sync_updates = enabled;
        // Scroll detection is gated on this: a scroll the renderer emits can
        // move cells that should have stayed put, and the repaint that fixes
        // them is only invisible inside a synchronized frame.
        self.renderer.set_sync_output(enabled);
    }

    /// Whether [synchronized output](Self::set_synchronized_output) frame
    /// wrapping is enabled.
    pub fn synchronized_output(&self) -> bool {
        self.sync_updates
    }

    /// Enable or disable the renderer's scroll-detection pass.
    ///
    /// On by default, and best left on: when a run of rows has simply moved,
    /// telling the terminal to move them costs a handful of bytes instead of
    /// a repaint.
    ///
    /// Detection additionally requires [synchronized
    /// output](Self::set_synchronized_output); see below. Turning this off
    /// gives up scrolling entirely, including on frames where it would have
    /// been safe.
    ///
    /// A fixed column is why that requirement exists. The scrolls uncurses
    /// emits are always full width: rows move with `SU`, `IL`/`DL` or a bare
    /// line feed,
    /// and the renderer does not set the left/right margins
    /// ([DECLRMM](crate::ansi::mode::Mode::LEFT_RIGHT_MARGIN) and
    /// [DECSLRM](crate::ansi::screen::write_set_left_right_margins)) that
    /// would confine them to a column range on a terminal supporting those.
    /// So a detected scroll moves that region too, and the renderer paints it
    /// back within the same frame. The end state is correct either way —
    /// which is why a test that compares the finished screen sees nothing
    /// wrong — but what the user sees is that region jumping and being put
    /// back, on every frame, for as long as they keep scrolling.
    ///
    /// Because that intermediate state is only hidden when the frame is
    /// presented in one step, detection runs **only** under
    /// [synchronized output](Self::set_synchronized_output), which wraps the
    /// frame in DEC 2026. Without it no scroll is emitted at all, whatever
    /// this setting says, and rows are redrawn directly instead.
    ///
    /// Synchronized output is off by default, and a
    /// [`Program`](crate::program::Program) turns it on only once the
    /// terminal has reported 2026 support, which takes an explicit
    /// [`query_capabilities`](crate::program::Program::query_capabilities).
    /// An application that never asks never gets scroll optimization,
    /// whatever its terminal supports.
    ///
    /// Detection is skipped outside [fullscreen](Self::set_fullscreen)
    /// regardless of this setting.
    ///
    /// This only sets state; it takes effect on the next
    /// [`render`](Self::render).
    pub fn set_scroll_optimize(&mut self, enabled: bool) {
        self.renderer.set_scroll_optimize(enabled);
    }

    /// Set the color profile used when emitting styled cells.
    pub fn set_color_profile(&mut self, profile: crate::color::Profile) {
        self.renderer.set_color_profile(profile);
    }

    /// Return the color profile used when emitting styled cells.
    ///
    /// This is the profile the renderer downsamples colors to, set by
    /// [`set_color_profile`](Self::set_color_profile) or detected from the
    /// environment by [`Program`](crate::program::Program). Pass it to
    /// [`Encode::encode_with`](crate::text::Encode::encode_with) to serialize
    /// a surface the same way this screen renders it.
    pub fn color_profile(&self) -> crate::color::Profile {
        self.renderer.color_profile()
    }

    /// Set the renderer optimization flags.
    ///
    /// [`TABS`](Optimizations::TABS) and [`BS`](Optimizations::BS) take
    /// effect immediately but do not persist across a raw-mode entry:
    /// [`Program::init`](crate::program::Program::init) and
    /// [`Program::resume`](crate::program::Program::resume) enable both,
    /// since raw mode is what makes them safe. Every other flag —
    /// including [`ONLCR`](Optimizations::ONLCR), which is opt-in and
    /// never granted — is left exactly as set here.
    pub fn set_optimizations(&mut self, optimizations: Optimizations) {
        self.renderer.set_optimizations(optimizations);
    }

    /// Return the renderer optimization flags currently in effect.
    pub fn optimizations(&self) -> Optimizations {
        self.renderer.optimizations()
    }

    // --- Render staging internals ---------------------------------------

    /// Stage a single rendered frame into [`out_buf`](Self::out_buf):
    /// synchronized-output begin, the renderer's cell diff, the optional
    /// declarative cursor move, synchronized-output end. Assumes the front
    /// buffer was synced.
    ///
    /// A visible cursor is hidden around the diff so it doesn't dance across
    /// cells as the renderer repositions it, *unless* synchronized output is
    /// enabled. A synchronized frame is presented in one step, so the cursor
    /// never visibly moves mid-frame; the hide/show pair is then skipped, both
    /// because it is redundant and because toggling DECTCEM every frame resets
    /// the cursor's blink phase, which reads as flicker. Whether to trust
    /// synchronized output is the caller's choice via
    /// [`set_synchronized_output`](Self::set_synchronized_output), not gated on
    /// detected capabilities.
    fn write_frame(&mut self) {
        let bracket_cursor = self.cursor_visible && !self.sync_updates;

        if self.sync_updates {
            mode::Mode::SYNCHRONIZED_OUTPUT
                .set(&mut self.out_buf)
                .unwrap();
        }
        if bracket_cursor {
            mode::Mode::CURSOR_VISIBLE.reset(&mut self.out_buf).unwrap();
        }

        self.renderer.render_back(&mut self.out_buf).unwrap();

        // Apply the declarative resting position (if any) inside the same
        // bracket as the cell diff, so the cursor lands atomically.
        // Sticky: re-applied every frame; move_to no-ops when already there.
        if let Some(pos) = self.desired_cursor {
            let pos = self.clamp_to_surface(pos);
            self.renderer
                .move_to(&mut self.out_buf, &self.front_buf, pos.y, pos.x)
                .unwrap();
        }

        if bracket_cursor {
            mode::Mode::CURSOR_VISIBLE.set(&mut self.out_buf).unwrap();
        }
        if self.sync_updates {
            mode::Mode::SYNCHRONIZED_OUTPUT
                .reset(&mut self.out_buf)
                .unwrap();
        }
    }

    /// Stage a cursor move without flushing. See
    /// [`move_cursor_to`](Self::move_cursor_to).
    pub(crate) fn stage_move_cursor_to(&mut self, target: Position) {
        let size = self.size();
        self.renderer
            .move_to_between_frames(&mut self.out_buf, size, target.y, target.x)
            .unwrap();
    }

    // --- Session handoff (driven by Program) -----------------------------

    /// Stage a move to the bottom row of the *last rendered* surface.
    ///
    /// Used before a shell handoff so the cursor lands below the managed
    /// area. Deliberately uses the last-render height rather than the live
    /// height, so a terminal that grew between the last render and the
    /// handoff does not push the cursor below where the user started.
    pub(crate) fn park_cursor(&mut self) -> io::Result<()> {
        let (last_width, last_height) = self.renderer.last_size();
        if last_height > 0 {
            let last = Size::new(last_width, last_height);
            self.renderer
                .move_to_between_frames(&mut self.out_buf, last, last_height - 1, 0)?;
        }
        Ok(())
    }

    /// Save the renderer's inline cursor anchor before the terminal switches
    /// to the alternate screen, so leaving it can restore the anchor.
    pub(crate) fn save_cursor(&mut self) {
        self.renderer.save_cursor();
    }

    /// Restore the inline cursor anchor saved by
    /// [`save_cursor`](Self::save_cursor).
    pub(crate) fn restore_cursor(&mut self) {
        self.renderer.restore_cursor();
    }

    /// Forget where the cursor is.
    ///
    /// The terminal is being handed back to the shell. Once it returns (e.g.
    /// after a suspend/resume, possibly with a resize that reflowed the
    /// surface), the tracked position is void; forget it so the next frame
    /// re-anchors at the current physical position instead of stepping up
    /// from a stale row and overwriting content above the surface.
    pub(crate) fn invalidate_cursor(&mut self) {
        self.renderer.invalidate_cursor();
    }
}

impl<W: Write> Write for Screen<W> {
    /// Append raw bytes to the staging buffer, ordered with any staged mode
    /// or frame bytes. They reach the writer on the next [`flush`](Self::flush).
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.out_buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    /// Drain the staging buffer to the writer and flush it.
    fn flush(&mut self) -> io::Result<()> {
        if !self.out_buf.is_empty() {
            #[cfg(debug_assertions)]
            crate::trace::tee_output(&self.out_buf);
            self.writer.write_all(&self.out_buf)?;
            self.out_buf.clear();
        }
        self.writer.flush()
    }
}

impl<W: Write> Bounded for Screen<W> {
    fn bounds(&self) -> Rect {
        self.front_buf.bounds()
    }
}

impl<W: Write> Surface for Screen<W> {
    fn cell(&self, pos: Position) -> Option<&Cell> {
        self.front_buf.cell(pos)
    }
}

impl<W: Write> SurfaceMut for Screen<W> {
    fn set_cell(&mut self, pos: Position, cell: &Cell) {
        self.front_buf.set_cell(pos, cell);
    }

    fn cell_mut(&mut self, pos: Position) -> Option<&mut Cell> {
        self.front_buf.cell_mut(pos)
    }

    fn insert_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.front_buf.insert_lines(y, n, bounds_bottom, fill);
    }

    fn delete_lines(&mut self, y: u16, n: u16, bounds_bottom: u16, fill: &Cell) {
        self.front_buf.delete_lines(y, n, bounds_bottom, fill);
    }

    fn insert_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        self.front_buf.insert_cells(pos, n, bounds_right, fill);
    }

    fn delete_cells(&mut self, pos: Position, n: u16, bounds_right: u16, fill: &Cell) {
        self.front_buf.delete_cells(pos, n, bounds_right, fill);
    }
}

impl<W: Write> TextSurface for Screen<W> {
    fn width_mode(&self) -> WidthMode {
        if self.grapheme_clusters {
            WidthMode::Grapheme
        } else {
            WidthMode::Wc
        }
    }

    fn eaw_wide(&self) -> bool {
        self.eaw_wide
    }
}

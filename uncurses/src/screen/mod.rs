//! [`Screen`] — a self-managing terminal application facade.
//!
//! `Screen<I, O>` bundles the three primitives a full-screen terminal
//! program needs into one owned handle:
//!
//! - a [`Terminal`] for the raw-mode lifecycle,
//! - a cell-diff renderer, and
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
//! session with [`Screen::init`], which enters raw mode and sends the
//! capability queries. Teardown is explicit: there is **no** `Drop`.
//! Hand the terminal back to the shell with [`Screen::finish`] (consume),
//! [`Screen::pause`] (keep, e.g. to shell out), or [`Screen::suspend`]
//! (pause, then stop the process with `SIGTSTP`); resume a
//! paused/suspended screen with [`Screen::resume`].
//!
//! ```no_run
//! use uncurses::screen::Screen;
//! use uncurses::style::Style;
//! use uncurses::text::TextSurface;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut screen = Screen::open()?; // build over /dev/tty
//! screen.init()?; // raw mode + capability queries
//! screen.enter_alt_screen()?;
//! screen.set_str((0, 0), "hello", Style::default());
//! screen.render()?;
//! let event = screen.read_event()?;
//! screen.observe_event(&event)?; // keep capability tracking alive
//! screen.finish()?; // restore the terminal
//! # Ok(())
//! # }
//! ```
//!
//! # Inline and fullscreen
//!
//! With the alternate screen on (after [`enter_alt_screen`](Screen::enter_alt_screen))
//! the managed area is the whole terminal viewport, addressed with absolute
//! moves. Without it (the default) the screen is *inline*: it occupies the
//! full terminal width but only as many rows as you draw, anchored in the
//! normal buffer so scrollback above and the returning shell prompt below
//! stay intact. Set the inline height with [`resize`](Screen::resize), and
//! push lines into the scrollback above the surface with
//! [`insert_above`](Screen::insert_above). Call
//! [`autoresize`](Screen::autoresize) to refit to the current window.
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
//!  Fullscreen (after enter_alt_screen): the whole viewport is the
//!  surface, addressed with absolute moves, and restored on exit.
//!
//!    ┌─────────────────────────────┐
//!    │                             │
//!    │  the whole terminal         │
//!    │  viewport is the surface    │
//!    │                             │
//!    └─────────────────────────────┘
//! ```
//!
//! # Options and defaults
//!
//! [`init`](Screen::init) uses [`ScreenOptions::default`];
//! [`init_with`](Screen::init_with) takes an explicit [`ScreenOptions`] to
//! choose the desired keyboard enhancements, whether to enable mouse
//! tracking at startup, and the in-band-resize and pixel-size behaviors.
//! Always-on defaults (such as bracketed paste) take effect immediately;
//! discovery-driven defaults are applied once the terminal answers the
//! capability queries (see [`capabilities`](Screen::capabilities)).
//!
//! [`Terminal`]: crate::terminal::Terminal
//! [`EventSource`]: crate::event::EventSource

mod cursor;
mod modes;
mod state;
#[cfg(test)]
mod tests;

pub use cursor::CursorShape;
pub use state::Capabilities;

/// Cell-diff capability flags controlling which optimized escape
/// sequences the screen's renderer may emit. Re-exported from the
/// renderer so applications can configure rendering with
/// [`Screen::set_optimizations`] without depending on renderer internals.
pub use crate::renderer::Optimizations;

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bitflags::bitflags;

use crate::ansi::{kitty, mode};
use crate::buffer::{Bounded, Surface, SurfaceMut};
use crate::cell::Cell;
use crate::color::Profile;
use crate::event::Input;
use crate::event::{Event, EventSource};
use crate::layout::{Position, Rect, Size};
use crate::renderer::{RenderBuffer, Renderer};
use crate::terminal::Terminal;
use crate::text::{TextSurface, WidthMode};

/// A self-managing terminal application facade composing a [`Terminal`],
/// a cell-diff renderer, and an [`EventSource`] with the non-render terminal and
/// input modes. See the [module documentation](self) for the lifecycle.
///
/// [`Terminal`]: crate::terminal::Terminal
/// [`EventSource`]: crate::event::EventSource
pub struct Screen<I, O>
where
    I: Input,
    O: Write,
{
    terminal: Terminal<I, O>,
    /// Caller-facing desired cell grid. Touched spans record where the
    /// application wrote since the last sync; the renderer filters them
    /// again against its staging buffer before diffing the terminal.
    front_buf: RenderBuffer,
    /// The diff renderer holding the tracked on-screen buffer, cursor model,
    /// fullscreen/relative-cursor layout, color profile, and optimizations.
    renderer: Renderer,
    /// Scratch byte buffer that drawing and mode methods stage escape bytes
    /// into before [`io::Write::flush`] drains them through the terminal.
    out_buf: Vec<u8>,
    /// Managed area width in cells.
    width: u16,
    /// Managed area height in cells.
    height: u16,
    /// East-Asian Ambiguous width policy used when measuring strings: when
    /// `true`, code points whose East-Asian-Width property is `Ambiguous`
    /// are measured as 2 cells instead of 1. See [`crate::text::char_width`].
    eaw_wide: bool,
    /// Input source behind the synchronous read path ([`Self::read_event`]
    /// and friends). Held in an `Arc<Mutex<_>>`; the lock is uncontended in
    /// the common single-reader case.
    source: Arc<Mutex<EventSource<I>>>,
    state: state::State,
    /// Terminal capabilities detected by intercepting the replies to the
    /// queries [`Self::init`] fires. Reads are pure; capability-report events
    /// are recorded here only when the caller feeds them back through
    /// [`Self::observe_event`].
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
    /// When the [`init`](Self::init) capability queries were written, used
    /// to bound the teardown drain that consumes their replies. `None`
    /// when no queries were sent (or once the drain has run).
    queries_sent_at: Option<Instant>,
}

/// Desired default behaviors applied by [`Screen::init_with`].
///
/// Always-on defaults (e.g. [`bracketed_paste`](Self::bracketed_paste))
/// take effect at init regardless of capability detection. Discovery-driven
/// defaults are applied once the terminating Primary DA reply confirms the
/// detected [`Capabilities`]; if the terminal never answers, only the
/// always-on defaults are in effect.
///
/// Most fields are app-level toggles you set to taste. Two are low-level
/// transport knobs that most applications should leave at their defaults:
/// [`request_pixel_size_on_resize`](Self::request_pixel_size_on_resize)
/// (when to re-query the window pixel size) and
/// [`query_drain_timeout`](Self::query_drain_timeout) (how long teardown
/// waits for capability replies). They exist for unusual terminals and
/// latency-sensitive teardown, not everyday configuration.
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
    /// [`KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES`]: crate::ansi::kitty::KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
    pub keyboard_enhancements: crate::ansi::kitty::KittyKeyboardFlags,
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
    /// Enable mouse tracking at init with the given [`MouseTracking`] extras
    /// (see [`Screen::enable_mouse`]). The request is emitted unconditionally;
    /// terminals ignore modes they do not support and degrade gracefully.
    /// Defaults to `None` (mouse tracking off).
    pub mouse: Option<MouseTracking>,
    /// Send terminal capability queries during
    /// [`init`](Screen::init_with).
    ///
    /// When `true` (the default), [`init_with`](Screen::init_with) probes the
    /// terminal for its keyboard, color, and feature support and waits for the
    /// replies, populating [`capabilities`](Screen::capabilities). Set it to
    /// `false` for output-only programs that draw frames and never read input:
    /// `init_with` still enters raw mode, sizes the managed area, and applies the
    /// environment-detected color profile, but emits no query escapes and
    /// waits for no replies, so the terminal is never probed and
    /// [`capabilities`](Screen::capabilities) stays at its env-derived
    /// defaults.
    pub query_capabilities: bool,
    /// How long teardown ([`finish`](Screen::finish) /
    /// [`pause`](Screen::pause)) will wait for the capability-query replies
    /// to arrive before restoring the terminal, so they cannot leak to the
    /// shell (or a child after `pause`) as stray input.
    ///
    /// The wait is measured from when [`init`](Screen::init) sent the
    /// queries and ends early as soon as the terminating Primary DA reply
    /// lands, so a responsive terminal costs only its round-trip and a
    /// long-running app pays nothing (the replies are consumed by the event
    /// loop, and the budget has long since elapsed). Only the rare path that
    /// tears down before the replies were consumed waits here. Defaults to
    /// 300ms.
    pub query_drain_timeout: Duration,
}

bitflags! {
    /// Optional mouse tracking features layered on top of basic button
    /// tracking.
    ///
    /// When mouse tracking is enabled, button-event tracking (presses,
    /// releases, and drags) and SGR encoding are always requested; these flags
    /// add optional extras on top. An empty set ([`MouseTracking::empty()`])
    /// means basic tracking with no extras.
    ///
    /// Mouse tracking is turned *off* through [`Screen::disable_mouse`] or by
    /// leaving [`ScreenOptions::mouse`] as `None`, not by an empty flag set.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct MouseTracking: u8 {
        /// Report pointer motion with no button held (any-event tracking).
        /// Adds hover motion on terminals that support it.
        const MOTION = 1 << 0;
        /// Request pixel coordinates (SGR-pixel). Terminals that support it
        /// report pixels; the rest fall back to SGR cell coordinates.
        const PIXELS = 1 << 1;
    }
}

impl Default for ScreenOptions {
    fn default() -> Self {
        Self {
            bracketed_paste: true,
            keyboard_enhancements:
                crate::ansi::kitty::KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES,
            prefer_in_band_resize: true,
            request_pixel_size_on_resize: cfg!(windows),
            mouse: None,
            query_capabilities: true,
            query_drain_timeout: Duration::from_millis(300),
        }
    }
}

impl<I, O> Screen<I, O>
where
    I: Input,
    O: Write,
{
    // --- Drawing -------------------------------------------------------

    /// Write `cell` at `pos` in the desired frame.
    pub fn set_cell(&mut self, pos: impl Into<Position>, cell: &Cell) {
        self.front_buf.set_cell(pos.into(), cell);
    }

    /// Borrow the cell at `pos` mutably, marking its columns touched.
    pub fn cell_mut(&mut self, pos: impl Into<Position>) -> Option<&mut Cell> {
        self.front_buf.cell_mut(pos.into())
    }

    /// Diff the staged frame against the tracked terminal, stage the
    /// minimal escape bytes, and flush them through the terminal.
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
    pub fn resize(&mut self, size: impl Into<Size>) {
        let size = size.into();
        self.width = size.width;
        self.height = size.height;
        self.front_buf.resize(size.width, size.height);
        self.renderer.request_clear();
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

    /// Insert `content` into the scrollback above the managed area and flush
    /// it to the terminal. In inline mode this pushes the lines into the
    /// terminal's scrollback; in alternate-screen mode they go into the alt
    /// screen's hidden scrollback. The managed area is preserved in place, so
    /// no redraw is needed and a following [`render`](Self::render) sees no
    /// change. An empty string is a no-op.
    ///
    /// # Errors
    ///
    /// Returns any error from flushing the inserted lines to the terminal.
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

    /// The managed area size in cells.
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    /// Immediately move the terminal cursor to a buffer-relative position and
    /// flush. No-op when the renderer already reports the cursor there with
    /// both axes known.
    ///
    /// This is imperative: the move is emitted and flushed now, independent of
    /// [`render`](Self::render). It does **not** affect the declarative resting
    /// position staged with [`set_cursor_position`](Self::set_cursor_position);
    /// a subsequent `render` will snap the cursor back to that sticky position
    /// if one is set. To change where frames leave the cursor, use
    /// `set_cursor_position` instead.
    pub fn move_cursor_to(&mut self, pos: impl Into<Position>) -> io::Result<()> {
        let target = pos.into();
        if self.renderer.cursor_known() && self.renderer.cursor_position() == target {
            return Ok(());
        }
        self.renderer
            .move_to(&mut self.out_buf, &self.front_buf, target.y, target.x)
            .unwrap();
        self.flush()
    }

    /// Immediately move the terminal cursor relative to the
    /// [tracked cursor](Self::tracked_cursor) and flush.
    ///
    /// Convenience over [`move_cursor_to`](Self::move_cursor_to): the target
    /// is the tracked cursor offset by `(dx, dy)`, saturating at the buffer
    /// origin. Like `move_cursor_to`, it does not clamp to the right or
    /// bottom edge. An unknown tracked cursor is treated as the origin.
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
    /// Use [`show_cursor`](Self::show_cursor) / [`hide_cursor`](Self::hide_cursor)
    /// for that. A position outside the managed area is clamped to its edges.
    ///
    /// The argument is anything that converts into a [`Position`], so a bare
    /// `(x, y)` works:
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// let mut screen = uncurses::screen::Screen::open()?;
    /// screen.set_cursor_position((4, 0)); // stage
    /// screen.clear_cursor_position();      // stop steering it
    /// # Ok(())
    /// # }
    /// ```
    pub fn set_cursor_position(&mut self, pos: impl Into<Position>) {
        self.state.desired_cursor = Some(pos.into());
    }

    /// Clear the staged cursor [resting position](Self::set_cursor_position),
    /// leaving the cursor wherever each frame's cell diff ends.
    pub fn clear_cursor_position(&mut self) {
        self.state.desired_cursor = None;
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
        match self.state.desired_cursor {
            Some(pos) => {
                let pos = self.clamp_to_surface(pos);
                !self.renderer.cursor_known() || self.renderer.cursor_position() != pos
            }
            None => false,
        }
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

    // --- Render-coupled mode toggles ------------------------------------

    /// Enter the alternate screen and flush.
    pub fn enter_alt_screen(&mut self) -> io::Result<()> {
        self.stage_set_alt_screen(true);
        self.flush()
    }

    /// Leave the alternate screen and flush.
    pub fn exit_alt_screen(&mut self) -> io::Result<()> {
        self.stage_set_alt_screen(false);
        self.flush()
    }

    /// Show the cursor and flush.
    pub fn show_cursor(&mut self) -> io::Result<()> {
        self.stage_set_cursor_visible(true);
        self.flush()
    }

    /// Hide the cursor and flush.
    pub fn hide_cursor(&mut self) -> io::Result<()> {
        self.stage_set_cursor_visible(false);
        self.flush()
    }

    /// Enable or disable synchronized-output frame wrapping.
    ///
    /// When enabled, each non-empty [`render`](Self::render) is wrapped in
    /// begin/end synchronized-output sequences (DEC mode 2026) so terminals
    /// that support it present the frame atomically, with no mid-frame
    /// repaint. Terminals that don't support 2026 ignore the markers.
    ///
    /// This is your switch to flip: uncurses does not second-guess it against
    /// detected capabilities. It is enabled automatically when the terminal
    /// reports 2026 support during [`init`](Self::init), and you can override
    /// that here at any time.
    ///
    /// Enabling it also changes how the cursor is handled per frame. With sync
    /// off, a visible cursor is hidden around the cell diff so it doesn't dance
    /// across cells as the renderer repositions it. With sync on, the frame is
    /// presented in one step, so that hide/show pair is dropped: it is
    /// redundant, and toggling the cursor every frame resets its blink phase,
    /// which reads as flicker.
    ///
    /// This only sets state; the markers are emitted on the next `render`.
    pub fn set_synchronized_output(&mut self, enabled: bool) {
        self.state.sync_updates = enabled;
    }

    /// Enable Unicode core / grapheme-cluster mode (DECSET 2027) and flush:
    /// [`set_str`](crate::text::TextSurface::set_str) and
    /// [`insert_above`](Self::insert_above) measure cell widths per extended
    /// grapheme cluster (UTS-29 plus emoji presentation rules).
    pub fn enable_grapheme_clusters(&mut self) -> io::Result<()> {
        self.stage_set_grapheme_clusters(true);
        self.flush()
    }

    /// Disable grapheme-cluster mode (DECRST 2027) and flush, falling back
    /// to per-code-point wcwidth-style measurement.
    pub fn disable_grapheme_clusters(&mut self) -> io::Result<()> {
        self.stage_set_grapheme_clusters(false);
        self.flush()
    }

    /// Set the per-screen kitty keyboard enhancements and flush.
    /// `Some(flags)` enables the selected progressive-enhancement bits;
    /// `None` disables every enhancement.
    pub fn set_kitty_keyboard(
        &mut self,
        flags: Option<crate::ansi::kitty::KittyKeyboardFlags>,
    ) -> io::Result<()> {
        let flags = flags.unwrap_or(crate::ansi::kitty::KittyKeyboardFlags::empty());
        self.stage_set_kitty_keyboard_flags(flags);
        self.flush()
    }

    /// Set the color profile used when emitting styled cells.
    pub fn set_color_profile(&mut self, profile: crate::color::Profile) {
        self.renderer.set_color_profile(profile);
    }

    /// Return the color profile used when emitting styled cells.
    ///
    /// This is the profile the renderer downsamples colors to, set by
    /// [`set_color_profile`](Self::set_color_profile) or detected from the
    /// environment when the screen was constructed. Pass it to
    /// [`Encode::encode_with`](crate::text::Encode::encode_with) to serialize
    /// a surface the same way this screen renders it.
    pub fn color_profile(&self) -> crate::color::Profile {
        self.renderer.color_profile()
    }

    /// Set the renderer optimization flags.
    pub fn set_optimizations(&mut self, optimizations: Optimizations) {
        self.renderer.set_optimizations(optimizations);
    }

    /// Return the renderer optimization flags currently in effect.
    pub fn optimizations(&self) -> Optimizations {
        self.renderer.optimizations()
    }

    /// Return whether Unicode core / grapheme-cluster mode (DEC 2027) is
    /// active. When `true`, text is measured per extended grapheme cluster;
    /// when `false`, per code point (wcwidth-style).
    pub fn grapheme_clusters(&self) -> bool {
        self.state.grapheme_clusters
    }

    /// Measure one extended grapheme cluster in terminal cells under the
    /// screen's current width mode and East-Asian Ambiguous policy.
    pub fn grapheme_width(&self, g: &str) -> u8 {
        TextSurface::width_mode(self).grapheme_width(g, self.eaw_wide)
    }

    /// Iterate a string as `(cluster, width)` pairs under the screen's
    /// current width mode and East-Asian Ambiguous policy.
    pub fn grapheme_cells<'a>(
        &self,
        s: &'a str,
    ) -> impl Iterator<Item = (&'a str, u8)> + use<'a, I, O> {
        crate::text::grapheme_cells(s, TextSurface::width_mode(self), self.eaw_wide)
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
        let bracket_cursor = self.state.cursor_visible && !self.state.sync_updates;

        if self.state.sync_updates {
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
        if let Some(pos) = self.state.desired_cursor {
            let pos = self.clamp_to_surface(pos);
            self.renderer
                .move_to(&mut self.out_buf, &self.front_buf, pos.y, pos.x)
                .unwrap();
        }

        if bracket_cursor {
            mode::Mode::CURSOR_VISIBLE.set(&mut self.out_buf).unwrap();
        }
        if self.state.sync_updates {
            mode::Mode::SYNCHRONIZED_OUTPUT
                .reset(&mut self.out_buf)
                .unwrap();
        }
    }

    /// Stage the alternate-screen toggle. Always emits DECSET/DECRST 1049;
    /// the bookkeeping side effects (save/restore the renderer cursor model,
    /// flip fullscreen/relative-cursor, request a clear, and re-apply the
    /// tracked Kitty keyboard flags onto the newly active buffer) run only on
    /// an actual transition.
    fn stage_set_alt_screen(&mut self, alt_screen: bool) {
        let changed = self.state.alt_screen != alt_screen;
        if alt_screen {
            if changed {
                self.renderer.save_cursor();
            }
            mode::Mode::ALT_SCREEN_SAVE_CURSOR
                .set(&mut self.out_buf)
                .unwrap();
            if changed {
                self.state.alt_screen = true;
                self.renderer.set_fullscreen(true);
                self.renderer.set_relative_cursor(false);
                self.renderer.request_clear();
            }
        } else {
            mode::Mode::ALT_SCREEN_SAVE_CURSOR
                .reset(&mut self.out_buf)
                .unwrap();
            if changed {
                self.state.alt_screen = false;
                self.renderer.set_fullscreen(false);
                self.renderer.set_relative_cursor(true);
                self.renderer.restore_cursor();
            }
        }
        // The kitty keyboard stack is per-screen-buffer; on an actual buffer
        // switch, re-apply the tracked flags onto the buffer we entered.
        if changed && !self.state.kitty_keyboard.is_empty() {
            kitty::write_set_kitty_keyboard(
                &mut self.out_buf,
                self.state.kitty_keyboard,
                kitty::KittyKeyboardMode::Set,
            )
            .unwrap();
        }
    }

    /// Stage a cursor-visibility change. Always emits the DECTCEM set/reset;
    /// the tracked state is updated to match.
    fn stage_set_cursor_visible(&mut self, visible: bool) {
        if visible {
            mode::Mode::CURSOR_VISIBLE.set(&mut self.out_buf).unwrap();
        } else {
            mode::Mode::CURSOR_VISIBLE.reset(&mut self.out_buf).unwrap();
        }
        self.state.cursor_visible = visible;
    }

    /// Stage a grapheme-cluster (DEC 2027) toggle. Always emits the
    /// DECSET/DECRST; the tracked state is updated to match.
    fn stage_set_grapheme_clusters(&mut self, enable: bool) {
        if enable {
            mode::Mode::UNICODE_CORE.set(&mut self.out_buf).unwrap();
        } else {
            mode::Mode::UNICODE_CORE.reset(&mut self.out_buf).unwrap();
        }
        self.state.grapheme_clusters = enable;
    }

    /// Stage a replacement Kitty keyboard enhancement flag set. Always emits
    /// the `CSI = flags ; 1 u` set; the tracked set is updated to match.
    fn stage_set_kitty_keyboard_flags(&mut self, flags: crate::ansi::kitty::KittyKeyboardFlags) {
        kitty::write_set_kitty_keyboard(
            &mut self.out_buf,
            flags,
            crate::ansi::kitty::KittyKeyboardMode::Set,
        )
        .unwrap();
        self.state.kitty_keyboard = flags;
    }

    // --- Event delegates -------------------------------------------------
    //
    // These are pure input reads: they lock the shared source, move bytes,
    // and hand back events. They do NOT track capabilities. Feed every event
    // you take back through [`observe_event`](Self::observe_event) so resize
    // handling and the discovery-driven defaults still apply — the sync and
    // async ([`event_stream`](Self::event_stream)) paths follow the same rule.

    /// Drive the input source for up to `timeout`, returning whether any
    /// event became available. A pure readiness wait: no capability tracking.
    /// See [`EventSource::poll`].
    pub fn poll_event(&self, timeout: Option<Duration>) -> io::Result<bool> {
        self.source.lock().unwrap().poll(timeout)
    }

    /// Take the next queued event without doing I/O. A pure read: pass the
    /// event to [`observe_event`](Self::observe_event) to keep capability
    /// tracking alive. See [`EventSource::try_read`].
    pub fn try_read_event(&self) -> Option<Event> {
        self.source.lock().unwrap().try_read()
    }

    /// Block until the next event. A pure read: pass the event to
    /// [`observe_event`](Self::observe_event) to keep capability tracking
    /// alive. See [`EventSource::read`].
    pub fn read_event(&self) -> io::Result<Event> {
        self.source.lock().unwrap().read()
    }

    /// Return an event to the front of the input queue, so the next
    /// [`read_event`](Self::read_event) / [`try_read_event`](Self::try_read_event)
    /// yields it before anything already queued. See [`EventSource::unread`].
    pub fn unread_event(&self, event: Event) {
        self.source.lock().unwrap().unread(event);
    }

    /// A shared handle to the input source behind
    /// [`read_event`](Self::read_event) and friends, for driving input from a
    /// separate reader over the same decoder rather than a second one racing
    /// the same file descriptor.
    ///
    /// The main use is async input: build an
    /// [`EventStream`](crate::event::EventStream) with
    /// [`EventStream::from_shared`](crate::event::EventStream::from_shared) from
    /// this handle and poll it on your executor. Like every read path, it is
    /// pure — feed each event back through [`observe_event`](Self::observe_event)
    /// to keep capability tracking alive.
    ///
    /// Sharing one source between a live reader and the screen's own
    /// [`read_event`](Self::read_event) is best-effort: an event goes to
    /// whichever consumer drains it first, so pick one reader in steady state.
    pub fn event_source(&self) -> Arc<Mutex<EventSource<I>>> {
        Arc::clone(&self.source)
    }

    /// Build an async [`EventStream`](crate::event::EventStream) over this
    /// screen's input, for reading events with `events.next().await` inside a
    /// `select!` on any executor. The stream shares the screen's decoder, so it
    /// does not race a second reader on the same file descriptor.
    ///
    /// Reads are pure: feed each event to [`observe_event`](Self::observe_event)
    /// to keep capability tracking alive, exactly as the sync path does. Read
    /// through the stream *or* through [`read_event`](Self::read_event) in
    /// steady state, not both at once: a shared source hands each event to
    /// whichever consumer drains it first.
    #[cfg(feature = "async")]
    pub fn event_stream(&self) -> crate::event::EventStream<I>
    where
        I: 'static,
    {
        crate::event::EventStream::from_shared(Arc::clone(&self.source))
    }

    /// Consume any still-pending replies to the capability queries
    /// [`init`](Self::init) fired, so they cannot leak to the shell (or a
    /// child after [`pause`](Self::pause)) as stray input once the terminal
    /// is restored to cooked mode.
    ///
    /// No-op unless queries were sent and their terminating Primary DA reply
    /// has not yet been observed. Otherwise it waits at most the time left in
    /// [`ScreenOptions::query_drain_timeout`], measured from when the queries
    /// were sent, and returns as soon as that Primary DA reply lands. Reusing
    /// the normal decode path means replies are consumed (not flushed), which
    /// is race-free and identical on every platform.
    fn drain_pending_queries(&mut self) -> io::Result<()> {
        if self.defaults_applied {
            return Ok(());
        }
        let Some(sent_at) = self.queries_sent_at.take() else {
            return Ok(());
        };
        let budget = self.options.query_drain_timeout;
        while !self.defaults_applied {
            let Some(remaining) = budget.checked_sub(sent_at.elapsed()) else {
                break;
            };
            if remaining.is_zero() {
                break;
            }
            // Wait up to the remaining budget for input, then decode whatever
            // arrived. Reads are pure now, so observe each event explicitly;
            // `observe_event` flips `defaults_applied` on the Primary DA reply
            // that terminates the capability-reply stream.
            if !self.poll_event(Some(remaining))? {
                break;
            }
            while let Some(ev) = self.try_read_event() {
                let _ = self.observe_event(&ev);
                if self.defaults_applied {
                    break;
                }
            }
        }
        Ok(())
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

    /// Apply an event to the screen's capability tracking. The event is
    /// inspected, never consumed.
    ///
    /// Reads are pure: [`read_event`](Self::read_event),
    /// [`try_read_event`](Self::try_read_event), and the async
    /// [`event_stream`](Self::event_stream) all hand back events *without*
    /// tracking capabilities. Pass every event you receive here — on both the
    /// sync and async paths — so capability detection stays alive.
    ///
    /// Capability-report replies to the queries [`init`](Self::init) fires are
    /// recorded into [`capabilities`](Self::capabilities), window-size reports
    /// update [`window_cells`](Self::window_cells) /
    /// [`window_pixels`](Self::window_pixels), and the render-affecting reports
    /// are applied. On the terminating Primary DA reply, the discovery-driven
    /// defaults from the active [`ScreenOptions`] are applied once (enabling
    /// mouse, keyboard enhancements, and in-band resize as configured), which
    /// may emit escapes to the terminal.
    ///
    /// ```ignore
    /// // Sync loop: read, observe, handle, render.
    /// loop {
    ///     let ev = screen.read_event()?;
    ///     screen.observe_event(&ev)?; // keep capability tracking alive
    ///     // ... handle ev ...
    ///     screen.render()?;
    /// }
    /// ```
    ///
    /// ```ignore
    /// // Async loop: same contract over an EventStream.
    /// use tokio_stream::StreamExt;
    ///
    /// let mut events = screen.event_stream();
    /// while let Some(ev) = events.next().await {
    ///     let ev = ev?;
    ///     screen.observe_event(&ev)?; // keep capability tracking alive
    ///     // ... handle ev ...
    ///     screen.render()?;
    /// }
    /// ```
    pub fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        use crate::ansi::mode::Mode;
        match *event {
            Event::ModeReport { mode, setting } if setting.is_available() => match mode {
                // Render-affecting: record and apply.
                Mode::SYNCHRONIZED_OUTPUT => {
                    self.caps.synchronized_output = true;
                    self.state.sync_updates = true;
                }
                Mode::UNICODE_CORE => {
                    self.caps.grapheme_clusters = true;
                    self.stage_set_grapheme_clusters(true);
                }
                // Recorded only; enabling is the app's choice.
                Mode::IN_BAND_RESIZE => self.caps.in_band_resize = true,
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
                // These come for free in the DA1 reply, which is sent as the
                // capability-query terminator regardless.
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
            // managed area is left to the app (call autoresize() as desired).
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
                self.renderer
                    .set_color_profile(crate::color::Profile::TrueColor);
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
            self.source.lock().unwrap().set_handle_resize(false);
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

        // Mouse tracking: enable exactly what the options request. enable_mouse
        // emits the mode set unconditionally; terminals that lack a requested
        // mode (e.g. SGR-pixel) ignore it and degrade to cell coordinates.
        if let Some(tracking) = self.options.mouse {
            self.enable_mouse(tracking)?;
        }
        Ok(())
    }

    /// Whether the host is Apple's `Terminal.app`, which does not support
    /// most of the queried features and mishandles the queries themselves.
    fn is_apple_terminal(&self) -> bool {
        self.terminal.get_env("TERM_PROGRAM").as_deref() == Some("Apple_Terminal")
    }

    /// The major version of Apple's `Terminal.app`, parsed from
    /// `TERM_PROGRAM_VERSION` (e.g. `"470"` or `"470.1"` yield `470`).
    /// `None` when the variable is absent or not numeric.
    fn apple_terminal_version(&self) -> Option<u32> {
        let raw = self.terminal.get_env("TERM_PROGRAM_VERSION")?;
        raw.split('.').next()?.trim().parse().ok()
    }

    /// Stage the initial capability queries into the output stream. Their
    /// replies arrive asynchronously through the normal event flow and are
    /// recorded as a side effect by [`observe`](Self::observe).
    ///
    /// The mode (DECRQM), XTVERSION, and XTGETTCAP queries are skipped on
    /// Apple's `Terminal.app`, which mishandles them. A Primary DA request
    /// is sent last so its reply marks the end of the capability replies.
    /// Detect the environment-derived color profile and apply it to the
    /// renderer. Called by `init_with` on every path so output downsamples
    /// correctly even when capability queries are skipped.
    /// Detect the environment-derived color profile and apply it to the
    /// renderer, clamping to no color when the output half is not a terminal
    /// (e.g. redirected to a file or pipe). `is_tty` is the output's
    /// terminal status; the caller supplies it since the platform handle
    /// bounds live on `init_with`.
    fn apply_env_color_profile(&mut self, is_tty: bool) {
        let profile = crate::color::Profile::detect_from(self.terminal.env(), is_tty);
        self.renderer.set_color_profile(profile);
    }

    /// Reconcile the terminal's hardware tab stops with the every-eight
    /// columns layout the renderer assumes whenever the `TABS`
    /// optimization is on. A prior program may have left arbitrary stops
    /// behind, which would make the `HT` (`\t`) moves the cursor planner
    /// emits land on the wrong columns. Modern terminals reset in one
    /// cursor-safe write via DECST8C; the rest get the portable
    /// TBC-then-HTS fallback. Skipped entirely when `TABS` is off, since
    /// the planner then never relies on tab stops. Staged and flushed so
    /// it reaches the terminal even when capability queries are disabled.
    fn reset_tab_stops(&mut self) -> io::Result<()> {
        if !self.optimizations().contains(Optimizations::TABS) {
            return Ok(());
        }
        if Optimizations::supports_decst8c(self.terminal.env()) {
            self.out_buf
                .write_all(crate::ansi::screen::SET_TAB_EVERY_8_COLUMNS)?;
        } else {
            crate::ansi::screen::write_reset_tab_stops_every_8(&mut self.out_buf, self.width)?;
        }
        self.flush()
    }

    fn send_init_queries(&mut self) -> io::Result<()> {
        use crate::ansi::ctrl::{REQUEST_PRIMARY_DA, REQUEST_XTVERSION};
        use crate::ansi::kitty::REQUEST_KITTY_KEYBOARD;
        use crate::ansi::mode::Mode;
        use crate::ansi::termcap::write_xtgettcap;
        use crate::color::Profile;

        // The env-derived profile is already applied by init_with via
        // apply_env_color_profile; read it back to decide whether there is
        // headroom to upgrade via XTGETTCAP.
        let profile = self.renderer.color_profile();

        // Always-safe queries.
        self.out_buf.write_all(REQUEST_KITTY_KEYBOARD)?;

        if !self.is_apple_terminal() {
            for mode in [
                Mode::SYNCHRONIZED_OUTPUT,
                Mode::UNICODE_CORE,
                Mode::IN_BAND_RESIZE,
                Mode::MOUSE_NORMAL,
                Mode::MOUSE_BUTTON,
                Mode::MOUSE_ANY,
                Mode::MOUSE_SGR,
                Mode::MOUSE_SGR_PIXEL,
            ] {
                mode.request(&mut self.out_buf)?;
            }
            self.out_buf.write_all(REQUEST_XTVERSION)?;
            self.out_buf
                .write_all(crate::ansi::xterm::QUERY_MODIFY_OTHER_KEYS)?;
            if profile < Profile::TrueColor {
                // One key per query: some terminals only answer the first
                // capability when several are batched in a single request.
                write_xtgettcap(&mut self.out_buf, &["RGB"])?;
                write_xtgettcap(&mut self.out_buf, &["Tc"])?;
            }
        } else {
            // Terminal.app mishandles the capability queries, but its
            // support for these features is known, so record them directly:
            // mouse tracking (normal/button/any) and the SGR encoding (no
            // pixel reporting). Bracketed paste is enabled unconditionally,
            // so it needs no capability flag.
            self.caps.mouse_normal = true;
            self.caps.mouse_button = true;
            self.caps.mouse_any = true;
            self.caps.mouse_sgr = true;
            // Terminal.app gained direct-color support in the build shipped
            // with macOS Tahoe; record it and upgrade the renderer when the
            // env-derived profile hasn't already.
            if profile < Profile::TrueColor
                && self.apple_terminal_version().is_some_and(|v| v >= 470)
            {
                self.caps.true_color = true;
                self.renderer.set_color_profile(Profile::TrueColor);
            }
        }

        self.out_buf.write_all(REQUEST_PRIMARY_DA)?;
        self.flush()
    }
}

impl<I, O> Write for Screen<I, O>
where
    I: Input,
    O: Write,
{
    /// Append raw bytes to the staging buffer, ordered with any staged mode
    /// or frame bytes. They reach the terminal on the next [`flush`](Self::flush).
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.out_buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    /// Drain the staging buffer through the terminal and flush it.
    fn flush(&mut self) -> io::Result<()> {
        if !self.out_buf.is_empty() {
            #[cfg(debug_assertions)]
            crate::trace::tee_output(&self.out_buf);
            self.terminal.write_all(&self.out_buf)?;
            self.out_buf.clear();
        }
        self.terminal.flush()
    }
}

impl<I, O> Bounded for Screen<I, O>
where
    I: Input,
    O: Write,
{
    fn bounds(&self) -> Rect {
        self.front_buf.bounds()
    }
}

impl<I, O> Surface for Screen<I, O>
where
    I: Input,
    O: Write,
{
    fn cell(&self, pos: Position) -> Option<&Cell> {
        self.front_buf.cell(pos)
    }
}

impl<I, O> SurfaceMut for Screen<I, O>
where
    I: Input,
    O: Write,
{
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

impl<I, O> TextSurface for Screen<I, O>
where
    I: Input,
    O: Write,
{
    fn width_mode(&self) -> WidthMode {
        if self.state.grapheme_clusters {
            WidthMode::Grapheme
        } else {
            WidthMode::Wc
        }
    }

    fn eaw_wide(&self) -> bool {
        self.eaw_wide
    }
}

impl<I, O> Screen<I, O>
where
    I: Input + Copy,
    O: Write,
{
    /// Build the render fields and event source over `terminal`, sizing the
    /// managed area to `size`. The color profile and renderer optimizations
    /// are detected from the terminal's captured environment. The terminal is
    /// left as-is.
    fn with_render(terminal: Terminal<I, O>, size: (u16, u16)) -> io::Result<Self> {
        let env = terminal.env();
        // Provisional profile; init_with reapplies it with the real
        // output-is-tty signal via apply_env_color_profile.
        let color_profile = Profile::detect_from(env, true);
        let optimizations = Optimizations::from_env(env);
        let mut renderer = Renderer::new();
        renderer.set_color_profile(color_profile);
        renderer.set_optimizations(optimizations);
        // Defaults match inline (no alt screen): the surface is anchored
        // wherever the cursor sits, so moves stay relative.
        renderer.set_fullscreen(false);
        renderer.set_relative_cursor(true);

        let source = Arc::new(Mutex::new(EventSource::new(terminal.input())?));
        let mut screen = Self {
            terminal,
            front_buf: RenderBuffer::new(0, 0),
            renderer,
            out_buf: Vec::with_capacity(4096),
            width: 0,
            height: 0,
            eaw_wide: false,
            source,
            state: state::State::default(),
            caps: Capabilities::default(),
            options: ScreenOptions::default(),
            defaults_applied: false,
            window_cells: None,
            window_pixels: None,
            terminal_name: None,
            queries_sent_at: None,
        };
        let (w, h) = size;
        if w != 0 || h != 0 {
            screen.resize((w, h));
        }
        Ok(screen)
    }
}

#[cfg(unix)]
impl<I, O> Screen<I, O>
where
    I: Input + Copy + std::os::fd::AsFd,
    O: Write + Copy + std::os::fd::AsFd,
{
    /// Construct a screen over `terminal` without touching the terminal:
    /// size the renderer to it and create an [`EventSource`] on its input
    /// half. The terminal is left as-is; call [`Self::init`] to enter raw
    /// mode and begin a session.
    pub fn new(terminal: Terminal<I, O>) -> io::Result<Self> {
        let ws = terminal.get_window_size()?;
        Self::with_render(terminal, (ws.col, ws.row))
    }

    /// Begin a session with the default [`ScreenOptions`]. See
    /// [`Self::init_with`].
    pub fn init(&mut self) -> io::Result<()> {
        self.init_with(ScreenOptions::default())
    }

    /// Begin a session: enter raw mode, apply the always-on defaults from
    /// `options`, and send the capability queries whose replies the event
    /// loop consumes. Discovery-driven defaults are applied later, once the
    /// terminating Primary DA reply confirms the detected capabilities (see
    /// [`Self::capabilities`]). Call once after [`Self::new`], before
    /// rendering.
    pub fn init_with(&mut self, options: ScreenOptions) -> io::Result<()> {
        self.options = options;
        self.terminal.make_raw()?;
        self.autoresize()?;
        // Apply the env color profile on every path so output downsamples
        // correctly even when capability queries are skipped. Disable color
        // when the output is not a terminal (redirected to a file or pipe).
        let is_tty = self.terminal.is_terminal().1;
        self.apply_env_color_profile(is_tty);
        self.reset_tab_stops()?;
        if self.options.bracketed_paste {
            self.enable_bracketed_paste()?;
        }
        if self.options.query_capabilities {
            self.send_init_queries()?;
            self.queries_sent_at = Some(Instant::now());
        }
        Ok(())
    }

    /// Query the current terminal window size (output half first, input as
    /// fallback). This is a live query; the cached
    /// [`window_cells`](Self::window_cells) /
    /// [`window_pixels`](Self::window_pixels) accessors return the
    /// last-observed values without I/O.
    pub fn get_window_size(&self) -> io::Result<crate::terminal::Winsize> {
        self.terminal.get_window_size()
    }

    /// Re-query the terminal size and resize the managed area to fit: the full
    /// terminal size in fullscreen (alternate screen on), or the terminal
    /// width with the current managed height preserved inline (alternate
    /// screen off). Refreshes the cached [`window_cells`](Self::window_cells)
    /// / [`window_pixels`](Self::window_pixels); on platforms whose size
    /// query reports no pixel size (e.g. the Windows console) the pixel size
    /// is requested over the wire.
    pub fn autoresize(&mut self) -> io::Result<()> {
        let Ok(ws) = self.terminal.get_window_size() else {
            // Keep the current size when the query fails rather than
            // collapsing the managed area to zero.
            return Ok(());
        };
        self.cache_window_size(ws)?;
        let height = if self.state.alt_screen {
            ws.row
        } else {
            self.height
        };
        self.resize((ws.col, height));
        Ok(())
    }

    /// Consume the screen and hand the terminal back to the shell: tear down
    /// every staged mode, reset the managed area, flush, and restore the
    /// terminal's prior state.
    pub fn finish(mut self) -> io::Result<()> {
        self.teardown()?;
        self.terminal.restore()
    }

    /// Hand the terminal back to the shell without consuming the screen,
    /// e.g. to run a child process. Re-enter with [`Self::resume`]. Like
    /// [`Self::finish`] but keeps the screen so the session can continue.
    pub fn pause(&mut self) -> io::Result<()> {
        self.teardown()?;
        self.terminal.restore()
    }

    /// Re-acquire the terminal after a [`Self::pause`] or [`Self::suspend`]:
    /// re-enter raw mode, refit the managed area to the current viewport, re-apply
    /// the saved render state and modes, and force a full repaint.
    pub fn resume(&mut self) -> io::Result<()> {
        self.terminal.make_raw()?;
        self.autoresize()?;
        self.restore()?;
        self.invalidate();
        self.flush()
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

    /// Hand the terminal back: consume any pending capability-query replies,
    /// reset every staged mode and the managed area to defaults, and flush. The
    /// caller restores the saved raw-mode state afterward.
    fn teardown(&mut self) -> io::Result<()> {
        self.drain_pending_queries()?;
        self.reset()?;
        self.flush()
    }
}

#[cfg(windows)]
impl<I, O> Screen<I, O>
where
    I: Input + Copy + std::os::windows::io::AsHandle,
    O: Write + Copy + std::os::windows::io::AsHandle,
{
    /// Construct a screen over `terminal` without touching the terminal:
    /// size the renderer to it and create an [`EventSource`] on its input
    /// half. The terminal is left as-is; call [`Self::init`] to enter raw
    /// mode and begin a session.
    pub fn new(terminal: Terminal<I, O>) -> io::Result<Self> {
        let ws = terminal.get_window_size()?;
        Self::with_render(terminal, (ws.col, ws.row))
    }

    /// Begin a session with the default [`ScreenOptions`]. See
    /// [`Self::init_with`].
    pub fn init(&mut self) -> io::Result<()> {
        self.init_with(ScreenOptions::default())
    }

    /// Begin a session: enter raw mode, apply the always-on defaults from
    /// `options`, and send the capability queries whose replies the event
    /// loop consumes. Discovery-driven defaults are applied later, once the
    /// terminating Primary DA reply confirms the detected capabilities (see
    /// [`Self::capabilities`]). Call once after [`Self::new`], before
    /// rendering.
    pub fn init_with(&mut self, options: ScreenOptions) -> io::Result<()> {
        self.options = options;
        self.terminal.make_raw()?;
        self.autoresize()?;
        // Apply the env color profile on every path so output downsamples
        // correctly even when capability queries are skipped. Disable color
        // when the output is not a terminal (redirected to a file or pipe).
        let is_tty = self.terminal.is_terminal().1;
        self.apply_env_color_profile(is_tty);
        self.reset_tab_stops()?;
        if self.options.bracketed_paste {
            self.enable_bracketed_paste()?;
        }
        if self.options.query_capabilities {
            self.send_init_queries()?;
            self.queries_sent_at = Some(Instant::now());
        }
        Ok(())
    }

    /// Query the current terminal window size (output half first, input as
    /// fallback). This is a live query; the cached
    /// [`window_cells`](Self::window_cells) /
    /// [`window_pixels`](Self::window_pixels) accessors return the
    /// last-observed values without I/O.
    pub fn get_window_size(&self) -> io::Result<crate::terminal::Winsize> {
        self.terminal.get_window_size()
    }

    /// Re-query the terminal size and resize the managed area to fit: the full
    /// terminal size in fullscreen (alternate screen on), or the terminal
    /// width with the current managed height preserved inline (alternate
    /// screen off). Refreshes the cached [`window_cells`](Self::window_cells)
    /// / [`window_pixels`](Self::window_pixels); on platforms whose size
    /// query reports no pixel size (e.g. the Windows console) the pixel size
    /// is requested over the wire.
    pub fn autoresize(&mut self) -> io::Result<()> {
        let Ok(ws) = self.terminal.get_window_size() else {
            // Keep the current size when the query fails rather than
            // collapsing the managed area to zero.
            return Ok(());
        };
        self.cache_window_size(ws)?;
        let height = if self.state.alt_screen {
            ws.row
        } else {
            self.height
        };
        self.resize((ws.col, height));
        Ok(())
    }

    /// Consume the screen and hand the terminal back to the shell: tear down
    /// every staged mode, reset the managed area, flush, and restore the
    /// terminal's prior state.
    pub fn finish(mut self) -> io::Result<()> {
        self.teardown()?;
        self.terminal.restore()
    }

    /// Hand the terminal back to the shell without consuming the screen,
    /// e.g. to run a child process. Re-enter with [`Self::resume`]. Like
    /// [`Self::finish`] but keeps the screen so the session can continue.
    pub fn pause(&mut self) -> io::Result<()> {
        self.teardown()?;
        self.terminal.restore()
    }

    /// Re-acquire the terminal after a [`Self::pause`]: re-enter raw mode,
    /// refit the managed area to the current viewport, re-apply the saved
    /// render state and modes, and force a full repaint.
    pub fn resume(&mut self) -> io::Result<()> {
        self.terminal.make_raw()?;
        self.autoresize()?;
        self.restore()?;
        self.invalidate();
        self.flush()
    }

    /// Hand the terminal back: consume any pending capability-query replies,
    /// reset every staged mode and the managed area to defaults, and flush. The
    /// caller restores the saved raw-mode state afterward.
    fn teardown(&mut self) -> io::Result<()> {
        self.drain_pending_queries()?;
        self.reset()?;
        self.flush()
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

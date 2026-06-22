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
//! use uncurses::text::TextSurface;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut screen = Screen::open()?; // build over /dev/tty
//! screen.init()?; // raw mode + staged queries
//! screen.enter_alt_screen()?;
//! screen.set_str((0, 0), "hello", Style::default());
//! screen.present()?;
//! let _event = screen.read_event()?;
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
//! # Async events
//!
//! With the `async` feature, `events` returns a
//! [`futures_core::Stream`] adapter that yields the same decoded events as
//! [`read_event`](Screen::read_event) (including the capability-detection side effect),
//! driven by a `next().await` loop. The stream borrows the screen only for
//! the duration of one poll, so the loop body is free to draw.
//!
//! [`Terminal`]: crate::terminal::Terminal
//! [`EventSource`]: crate::event::EventSource
//! [`futures_core::Stream`]: https://docs.rs/futures-core/latest/futures_core/stream/trait.Stream.html

mod cursor;
mod modes;
mod mouse;
mod state;
#[cfg(test)]
mod tests;

pub use cursor::CursorShape;
pub use state::Capabilities;

/// Cell-diff capability flags controlling which optimized escape
/// sequences the screen's renderer may emit. Re-exported from the
/// renderer so applications can configure rendering with
/// [`Screen::use_optimizations`] without depending on renderer internals.
pub use crate::renderer::Optimizations;

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::ansi::{kitty, mode};
use crate::buffer::{Bounded, Surface, SurfaceMut};
use crate::cell::Cell;
use crate::color::Profile;
#[cfg(feature = "async")]
use crate::event::EventStream;
use crate::event::source::Input;
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
    /// Input source, shared so the synchronous read path ([`Self::read`]
    /// and friends) and the async [`EventStream`](Self::events) can both
    /// drive it. The lock is uncontended in the sync-only case.
    source: Arc<Mutex<EventSource<I>>>,
    /// Thread-backed event stream, created lazily on the first
    /// [`events`](Self::events) call and reused thereafter.
    #[cfg(feature = "async")]
    stream: Option<EventStream<I>>,
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
    /// Enable mouse tracking at init with the given motion/pixel preference
    /// (see [`Screen::enable_mouse`]). The screen picks the best mode and
    /// encoding the terminal supports once capabilities are known. Defaults
    /// to `None` (mouse tracking off).
    pub mouse: Option<MousePreference>,
    /// Send terminal capability queries during
    /// [`init`](Screen::init_with).
    ///
    /// When `true` (the default), [`init_with`](Screen::init_with) probes the
    /// terminal for its keyboard, color, and feature support and waits for the
    /// replies, populating [`capabilities`](Screen::capabilities). Set it to
    /// `false` for output-only programs that draw frames and never read input:
    /// `init_with` still enters raw mode, sizes the canvas, and applies the
    /// environment-detected color profile, but emits no query escapes and
    /// waits for no replies, so the terminal is never probed and
    /// [`capabilities`](Screen::capabilities) stays at its env-derived
    /// defaults.
    pub query_capabilities: bool,
}

/// Mouse tracking preference for [`ScreenOptions::mouse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MousePreference {
    /// Report pointer motion (button-event or any-event tracking) rather
    /// than presses and releases only.
    pub motion: bool,
    /// Request coordinates in pixels rather than cells, when the terminal
    /// supports pixel reporting.
    pub pixels: bool,
}

impl Default for ScreenOptions {
    fn default() -> Self {
        Self {
            bracketed_paste: true,
            keyboard_enhancements: crate::ansi::KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES,
            prefer_in_band_resize: true,
            request_pixel_size_on_resize: cfg!(windows),
            mouse: None,
            query_capabilities: true,
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

    /// Diff the staged frame against the tracked terminal and stage the
    /// minimal escape bytes. Infallible; call [`flush`](Self::flush) or
    /// [`present`](Self::present) to send them.
    pub fn render(&mut self) {
        if !self.renderer.sync_front(&mut self.front_buf) {
            return;
        }
        self.write_frame();
    }

    /// Render the next frame and flush the staged bytes through the
    /// terminal in one call.
    pub fn present(&mut self) -> io::Result<()> {
        self.render();
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

    /// Insert `content` into the scrollback above the managed area, then
    /// force a full redraw on the next [`render`](Self::render). In inline
    /// mode this pushes the lines into the terminal's scrollback; in
    /// alternate-screen mode they go into the alt screen's hidden
    /// scrollback. An empty string is a no-op.
    pub fn insert_above(&mut self, content: &str) {
        if content.is_empty() {
            return;
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
        let width_mode = TextSurface::width_mode(self);
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
        self.renderer.request_clear();
    }

    /// The managed area size in cells.
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    /// Stage a cursor move to a buffer-relative position. No-op when the
    /// renderer already reports the cursor there with both axes known.
    pub fn move_cursor_to(&mut self, pos: impl Into<Position>) {
        let target = pos.into();
        if self.renderer.cursor_known() && self.renderer.cursor_position() == target {
            return;
        }
        self.renderer
            .move_to(&mut self.out_buf, &self.front_buf, target.y, target.x)
            .unwrap();
    }

    /// The renderer's tracked cursor position: the buffer-relative cell
    /// where the renderer believes the terminal cursor currently sits. This
    /// is bookkeeping, not a live cursor-position query.
    pub fn tracked_cursor_position(&self) -> Position {
        self.renderer.cursor_position()
    }

    /// Mark the tracked cursor position unknown, so the next staged move
    /// always emits rather than short-circuiting on a matching tracked
    /// position. Use after moving the terminal cursor by a means the
    /// renderer cannot see (e.g. a raw escape written directly).
    pub fn invalidate_tracked_cursor(&mut self) {
        self.renderer.invalidate_cursor();
    }

    /// Assume the tracked cursor is at buffer-relative `pos`, with both
    /// axes known, *without* emitting any move. This only updates the
    /// renderer's belief; the caller must have already placed the terminal
    /// cursor there (e.g. with a raw escape the renderer cannot see). For an
    /// actual cursor move use [`move_cursor_to`](Self::move_cursor_to).
    pub fn assume_cursor_position(&mut self, pos: impl Into<Position>) {
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

    /// Enable synchronized-output frame wrapping: each non-empty
    /// [`render`](Self::render) is wrapped in begin/end synchronized-output
    /// sequences so terminals that support DECSET 2026 treat a frame as
    /// atomic. Takes effect on the next frame that has render work.
    pub fn enable_synchronized_output(&mut self) -> io::Result<()> {
        self.state.sync_updates = true;
        Ok(())
    }

    /// Disable synchronized-output frame wrapping.
    pub fn disable_synchronized_output(&mut self) -> io::Result<()> {
        self.state.sync_updates = false;
        Ok(())
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
        flags: Option<crate::ansi::KittyKeyboardFlags>,
    ) -> io::Result<()> {
        let flags = flags.unwrap_or(crate::ansi::KittyKeyboardFlags::NONE);
        self.stage_set_kitty_keyboard_flags(flags);
        self.flush()
    }

    /// Set the color profile used when emitting styled cells.
    pub fn use_color_profile(&mut self, profile: crate::color::Profile) {
        self.renderer.set_color_profile(profile);
    }

    /// Return the color profile used when emitting styled cells.
    ///
    /// This is the profile the renderer downsamples colors to, set by
    /// [`use_color_profile`](Self::use_color_profile) or detected from the
    /// environment when the screen was constructed. Pass it to
    /// [`Encode::encode_with`](crate::text::Encode::encode_with) to serialize
    /// a surface the same way this screen renders it.
    pub fn color_profile(&self) -> crate::color::Profile {
        self.renderer.color_profile()
    }

    /// Set the renderer optimization flags.
    pub fn use_optimizations(&mut self, optimizations: Optimizations) {
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
    /// synchronized-output begin, cursor hide (so the cursor doesn't dance
    /// across cells during the diff), the renderer's cell diff, cursor show,
    /// synchronized-output end. Assumes the front buffer was synced.
    fn write_frame(&mut self) {
        if self.state.sync_updates {
            mode::Mode::SYNCHRONIZED_OUTPUT
                .set(&mut self.out_buf)
                .unwrap();
        }
        if self.state.cursor_visible {
            mode::Mode::CURSOR_VISIBLE.reset(&mut self.out_buf).unwrap();
        }

        self.renderer.render_back(&mut self.out_buf).unwrap();

        if self.state.cursor_visible {
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
    fn stage_set_kitty_keyboard_flags(&mut self, flags: crate::ansi::KittyKeyboardFlags) {
        kitty::write_set_kitty_keyboard(
            &mut self.out_buf,
            flags,
            crate::ansi::KittyKeyboardMode::Set,
        )
        .unwrap();
        self.state.kitty_keyboard = flags;
    }

    // --- Event delegates -------------------------------------------------

    /// Drive the input source for up to `timeout`, returning whether any
    /// event became available. See [`EventSource::poll`].
    pub fn poll_event(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        self.source.lock().unwrap().poll(timeout)
    }

    /// Take the next queued event without doing I/O. Capability reports are
    /// recorded as a side effect but still returned. See
    /// [`EventSource::try_read`].
    pub fn try_read_event(&mut self) -> Option<Event> {
        let ev = self.source.lock().unwrap().try_read()?;
        // A failed flush while applying discovery-driven defaults is
        // best-effort here; it resurfaces on the next explicit flush.
        let _ = self.observe(&ev);
        Some(ev)
    }

    /// Block until the next event. Capability reports are recorded as a
    /// side effect but still returned. See [`EventSource::read`].
    pub fn read_event(&mut self) -> io::Result<Event> {
        let ev = self.source.lock().unwrap().read()?;
        self.observe(&ev)?;
        Ok(ev)
    }

    /// Return an event to the front of the input queue, so the next
    /// [`read_event`](Self::read_event) / [`try_read_event`](Self::try_read_event)
    /// yields it before anything already queued. See [`EventSource::unread`].
    pub fn unread_event(&mut self, event: Event) {
        self.source.lock().unwrap().unread(event);
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

        // Mouse tracking: enable with the requested motion/pixel preference,
        // letting the screen pick the best mode and encoding the terminal
        // supports now that capabilities are known. Pixel coordinates are
        // requested only when SGR-pixel encoding is actually available, so the
        // preference degrades to cell coordinates on terminals without it.
        if let Some(pref) = self.options.mouse {
            self.enable_mouse(pref.motion, pref.pixels && self.caps.mouse_sgr_pixel)?;
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
    /// intercepted by the event delegates (see [`Self::intercept`]).
    ///
    /// The mode (DECRQM), XTVERSION, and XTGETTCAP queries are skipped on
    /// Apple's `Terminal.app`, which mishandles them. A Primary DA request
    /// is sent last so its reply marks the end of the capability replies.
    /// Detect the environment-derived color profile and apply it to the
    /// renderer. Called by `init_with` on every path so output downsamples
    /// correctly even when capability queries are skipped.
    fn apply_env_color_profile(&mut self) {
        let profile = crate::color::Profile::detect_from(self.terminal.env(), true);
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

    fn stage_init_queries(&mut self) -> io::Result<()> {
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

#[cfg(feature = "async")]
impl<I, O> Screen<I, O>
where
    I: Input + 'static,
    O: Write,
{
    /// An async event stream that yields decoded events and runs the same
    /// capability detection ([`observe`](Self::read_event)) as the synchronous
    /// [`read_event`](Self::read_event) path.
    ///
    /// The thread-backed stream is created on the first call and reused
    /// thereafter; the helper thread waits for input readiness and wakes the
    /// polling task. Drive it with a `Stream` extension trait's `next`:
    ///
    /// ```ignore
    /// while let Some(ev) = screen.events().next().await {
    ///     let ev = ev?;
    ///     // draw with `screen` here — the events() borrow has ended
    /// }
    /// ```
    ///
    /// The returned [`Events`] borrows the screen for the duration of one
    /// `next().await`, so it cannot be bound across the loop; call
    /// `screen.events().next()` each iteration (the underlying stream and
    /// its thread persist on the screen).
    pub fn events(&mut self) -> Events<'_, I, O> {
        if self.stream.is_none() {
            self.stream = Some(EventStream::from_shared(Arc::clone(&self.source)));
        }
        Events { screen: self }
    }
}

/// Async event stream adapter returned by [`Screen::events`].
///
/// Implements [`futures_core::Stream`], yielding `io::Result<Event>` and
/// running [`Screen`] capability detection on each event before yielding it.
/// Borrows the screen for the duration of a single poll.
#[cfg(feature = "async")]
pub struct Events<'a, I, O>
where
    I: Input + 'static,
    O: Write,
{
    screen: &'a mut Screen<I, O>,
}

#[cfg(feature = "async")]
impl<I, O> futures_core::Stream for Events<'_, I, O>
where
    I: Input + 'static,
    O: Write,
{
    type Item = io::Result<Event>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;

        let screen = &mut self.get_mut().screen;
        // Scope the stream-field borrow so it ends before `observe` borrows
        // the whole screen. The yielded `Poll` owns its event, holding no
        // borrow of the stream.
        let polled = {
            let stream = screen
                .stream
                .as_mut()
                .expect("stream is created in events()");
            std::pin::Pin::new(stream).poll_next(cx)
        };
        match polled {
            Poll::Ready(Some(Ok(ev))) => match screen.observe(&ev) {
                Ok(()) => Poll::Ready(Some(Ok(ev))),
                Err(e) => Poll::Ready(Some(Err(e))),
            },
            other => other,
        }
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
            #[cfg(feature = "async")]
            stream: None,
            state: state::State::default(),
            caps: Capabilities::default(),
            options: ScreenOptions::default(),
            defaults_applied: false,
            window_cells: None,
            window_pixels: None,
            terminal_name: None,
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
    /// `options`, and stage the capability queries whose replies the event
    /// loop consumes. Discovery-driven defaults are applied later, once the
    /// terminating Primary DA reply confirms the detected capabilities (see
    /// [`Self::capabilities`]). Call once after [`Self::new`], before
    /// rendering.
    pub fn init_with(&mut self, options: ScreenOptions) -> io::Result<()> {
        self.options = options;
        self.terminal.make_raw()?;
        self.autoresize()?;
        // Apply the env color profile on every path so output downsamples
        // correctly even when capability queries are skipped.
        self.apply_env_color_profile();
        self.reset_tab_stops()?;
        if self.options.bracketed_paste {
            self.enable_bracketed_paste()?;
        }
        if self.options.query_capabilities {
            self.stage_init_queries()?;
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
        let height = if self.state.alt_screen {
            ws.row
        } else {
            self.height
        };
        self.resize((ws.col, height));
        Ok(())
    }

    /// Consume the screen and hand the terminal back to the shell: stop the
    /// async event stream if one is running, tear down every staged mode,
    /// reset the canvas, flush, and restore the terminal's prior state.
    pub fn finish(mut self) -> io::Result<()> {
        // Stop the async event stream's helper thread, if one was started.
        // Consuming `self` would drop it anyway, but doing it explicitly here
        // marks the difference from `pause`, which keeps the stream alive for
        // `resume`.
        #[cfg(feature = "async")]
        drop(self.stream.take());
        self.stage_teardown()?;
        self.terminal.restore()
    }

    /// Hand the terminal back to the shell without consuming the screen,
    /// e.g. to run a child process. Re-enter with [`Self::resume`]. Like
    /// [`Self::finish`] but keeps the screen so the session can continue.
    /// Any running async event stream is stopped first, so its reader thread
    /// does not compete with the child for input; the next
    /// [`events`](Self::events) call after [`resume`](Self::resume) starts a
    /// fresh one.
    pub fn pause(&mut self) -> io::Result<()> {
        // Stop the async reader thread before handing off the terminal. The
        // stream is recreated lazily by the next `events()` call.
        #[cfg(feature = "async")]
        {
            self.stream = None;
        }
        self.stage_teardown()?;
        self.terminal.restore()
    }

    /// Re-acquire the terminal after a [`Self::pause`] or [`Self::suspend`]:
    /// re-enter raw mode, refit the canvas to the current viewport, re-apply
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

    fn stage_teardown(&mut self) -> io::Result<()> {
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
    /// `options`, and stage the capability queries whose replies the event
    /// loop consumes. Discovery-driven defaults are applied later, once the
    /// terminating Primary DA reply confirms the detected capabilities (see
    /// [`Self::capabilities`]). Call once after [`Self::new`], before
    /// rendering.
    pub fn init_with(&mut self, options: ScreenOptions) -> io::Result<()> {
        self.options = options;
        self.terminal.make_raw()?;
        self.autoresize()?;
        // Apply the env color profile on every path so output downsamples
        // correctly even when capability queries are skipped.
        self.apply_env_color_profile();
        self.reset_tab_stops()?;
        if self.options.bracketed_paste {
            self.enable_bracketed_paste()?;
        }
        if self.options.query_capabilities {
            self.stage_init_queries()?;
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
        let height = if self.state.alt_screen {
            ws.row
        } else {
            self.height
        };
        self.resize((ws.col, height));
        Ok(())
    }

    /// Consume the screen and hand the terminal back to the shell: stop the
    /// async event stream if one is running, tear down every staged mode,
    /// reset the canvas, flush, and restore the terminal's prior state.
    pub fn finish(mut self) -> io::Result<()> {
        // Stop the async event stream's helper thread, if one was started.
        // Consuming `self` would drop it anyway, but doing it explicitly here
        // marks the difference from `pause`, which keeps the stream alive for
        // `resume`.
        #[cfg(feature = "async")]
        drop(self.stream.take());
        self.stage_teardown()?;
        self.terminal.restore()
    }

    /// Hand the terminal back to the shell without consuming the screen,
    /// e.g. to run a child process. Re-enter with [`Self::resume`]. Like
    /// [`Self::finish`] but keeps the screen so the session can continue.
    /// Any running async event stream is stopped first, so its reader thread
    /// does not compete with the child for input; the next
    /// [`events`](Self::events) call after [`resume`](Self::resume) starts a
    /// fresh one.
    pub fn pause(&mut self) -> io::Result<()> {
        // Stop the async reader thread before handing off the terminal. The
        // stream is recreated lazily by the next `events()` call.
        #[cfg(feature = "async")]
        {
            self.stream = None;
        }
        self.stage_teardown()?;
        self.terminal.restore()
    }

    /// Re-acquire the terminal after a [`Self::pause`]: re-enter raw mode,
    /// refit the canvas to the current viewport, re-apply the saved
    /// render state and modes, and force a full repaint.
    pub fn resume(&mut self) -> io::Result<()> {
        self.terminal.make_raw()?;
        self.autoresize()?;
        self.restore()?;
        self.invalidate();
        self.flush()
    }

    fn stage_teardown(&mut self) -> io::Result<()> {
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

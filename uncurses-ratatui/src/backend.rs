use std::io::{self, Write};
use std::sync::{Arc, Mutex, MutexGuard};

use ratatui::Viewport;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell as RtCell;
use ratatui::layout::{Position as RtPosition, Size as RtSize};
use uncurses::buffer::SurfaceMut;
use uncurses::canvas::Canvas;
use uncurses::cell::Cell as CzCell;
use uncurses::event::{EventSource, Input};
use uncurses::terminal::Terminal;
use uncurses::terminal::{State, Stdin, Stdout, TtyInput, TtyOutput};

use crate::convert::cell_from_ratatui;

/// Platform bound for an output half usable as a real terminal: writable,
/// `Copy` (so the `Terminal` can hand copies to the screen while keeping
/// its own), and exposing the OS handle the raw-mode and window-size
/// syscalls need. `Stdout`/`TtyOutput` satisfy it.
#[cfg(unix)]
pub trait OutputHandle: Write + Copy + std::os::fd::AsFd {}
#[cfg(unix)]
impl<T: Write + Copy + std::os::fd::AsFd> OutputHandle for T {}
#[cfg(windows)]
pub trait OutputHandle: Write + Copy + std::os::windows::io::AsHandle {}
#[cfg(windows)]
impl<T: Write + Copy + std::os::windows::io::AsHandle> OutputHandle for T {}

/// Default geometry when the terminal size cannot be read at construction.
const DEFAULT_SIZE: (u16, u16) = (80, 24);

/// A `ratatui` [`Backend`](ratatui::backend::Backend) that owns the whole
/// terminal stack: the [`Terminal`] handle (raw-mode lifecycle, window
/// size), the [`Canvas`] it renders through, and a shared [`EventSource`]
/// for input.
///
/// Side-effecting methods (`hide_cursor`, `show_cursor`,
/// `set_cursor_position`, `clear`, `clear_region`) flush immediately,
/// while `draw` only updates the back buffer and defers I/O to the next
/// `flush`.
///
/// # Events
///
/// Read synchronously by locking [`events`](Self::events), or take an
/// asynchronous [`EventStream`](uncurses::event::EventStream) with
/// [`event_stream`](Self::event_stream) (the `async` feature) over the
/// same shared source. Synchronous reads keep working alongside a live
/// stream, subject to the stream's coexistence caveats.
///
/// # Setup
///
/// Construction is explicit: it does not enter raw mode or the alternate
/// screen. Call [`make_raw`](Self::make_raw) and the relevant
/// [`Canvas`](Self::screen_mut) mode setters yourself, and
/// [`restore`](Self::restore) on exit.
pub struct UncursesBackend<I: Input, O: Write> {
    terminal: Terminal<I, O>,
    screen: Canvas<O>,
    events: Arc<Mutex<EventSource<I>>>,
    /// The ratatui viewport, set via [`set_viewport`](Self::set_viewport)
    /// (by `init_with_options`). Determines the screen buffer height
    /// (inline height vs full terminal height) and whether `draw` /
    /// `set_cursor_position` translate absolute rows into the inline
    /// region.
    viewport: Viewport,
    /// Top row of an inline viewport. Seeded at initial setup from
    /// [`get_cursor_position`](Backend::get_cursor_position), then kept
    /// exact across resizes by [`clear_region`](Backend::clear_region),
    /// which observes the cursor ratatui parks at the recomputed viewport
    /// top before clearing. Absolute rows are translated down by this when
    /// rendering an inline viewport.
    inline_origin: u16,
    /// Last full terminal size observed by [`size`](Backend::size) /
    /// [`window_size`](Backend::window_size). When it changes the screen
    /// is marked stale (`size_dirty`) so the next [`draw`](Backend::draw)
    /// repaints in full. Tracked behind `Cell` because `size` takes
    /// `&self`.
    last_size: std::cell::Cell<(u16, u16)>,
    /// Set when `last_size` changes; consumed by `draw` to invalidate the
    /// screen.
    size_dirty: std::cell::Cell<bool>,
    /// Absolute row last requested by
    /// [`set_cursor_position`](Backend::set_cursor_position). When ratatui
    /// recomputes an inline viewport (initial setup and every resize) it
    /// positions the cursor at the viewport's top row before clearing it,
    /// so [`clear_region`](Backend::clear_region) reads this back as the
    /// fresh `inline_origin` — the only place the *true* viewport top is
    /// observable (the cursor reported by `get_cursor_position` sits at the
    /// app's cursor, which need not be the viewport top).
    last_cursor_row: u16,
}

impl UncursesBackend<Stdin, Stdout> {
    /// Build a backend over the process stdio (`stdin` + `stdout`).
    pub fn stdio() -> io::Result<Self> {
        Self::from_terminal(Terminal::stdio())
    }
}

impl UncursesBackend<TtyInput, TtyOutput> {
    /// Build a backend over the controlling terminal (`/dev/tty`, or
    /// `CONIN$`/`CONOUT$` on Windows), bypassing redirected stdio.
    pub fn open() -> io::Result<Self> {
        Self::from_terminal(Terminal::open()?)
    }
}

impl<I, O> UncursesBackend<I, O>
where
    I: Input + Copy + 'static,
    O: OutputHandle,
{
    /// Assemble a backend from a `Terminal`, building the source and
    /// screen from its `Copy` halves. The screen is sized from the
    /// terminal's current window size, falling back to 80x24.
    fn from_terminal(terminal: Terminal<I, O>) -> io::Result<Self> {
        let size = terminal
            .get_window_size()
            .map(|w| (w.col, w.row))
            .ok()
            .filter(|&(c, r)| c != 0 && r != 0)
            .unwrap_or(DEFAULT_SIZE);
        let screen = Canvas::new(terminal.output(), size);
        let events = EventSource::new(terminal.input())?;
        Ok(Self::new(terminal, events, screen))
    }
}

impl<I, O> UncursesBackend<I, O>
where
    I: Input,
    O: Write,
{
    /// Build a backend from pre-constructed parts, so a caller can reuse
    /// an existing terminal, source, and screen. The source is shared
    /// behind `Arc<Mutex<_>>` so it can be read synchronously and, under
    /// the `async` feature, also back an [`EventStream`].
    pub fn new(terminal: Terminal<I, O>, events: EventSource<I>, screen: Canvas<O>) -> Self {
        Self {
            terminal,
            screen,
            events: Arc::new(Mutex::new(events)),
            viewport: Viewport::Fullscreen,
            inline_origin: 0,
            last_size: std::cell::Cell::new((0, 0)),
            size_dirty: std::cell::Cell::new(false),
            last_cursor_row: 0,
        }
    }

    /// Record an observed full terminal size; if it differs from the last,
    /// flag the screen stale so the next [`draw`](Backend::draw) repaints
    /// in full. Takes `&self` so `size`/`window_size` can call it.
    fn note_size(&self, size: (u16, u16)) {
        if self.last_size.get() != size {
            self.last_size.set(size);
            self.size_dirty.set(true);
        }
    }

    /// Set the ratatui viewport. Called by
    /// [`init_with_options`](crate::init_with_options) so the backend can
    /// size its screen buffer (inline height vs full terminal height) and
    /// translate coordinates for an inline viewport. The default is
    /// [`Viewport::Fullscreen`].
    ///
    /// For an inline viewport the screen buffer is resized to the inline
    /// height immediately (clamped to the current terminal height), so the
    /// first render is already the right size rather than starting at full
    /// height and shrinking on the first draw.
    pub fn set_viewport(&mut self, viewport: Viewport) {
        if let Viewport::Inline(height) = viewport {
            let h = height.min(self.screen.height());
            self.screen.resize(self.screen.width(), h);
        }
        self.viewport = viewport;
    }

    /// Borrow the terminal handle.
    pub fn terminal(&self) -> &Terminal<I, O> {
        &self.terminal
    }

    /// Mutably borrow the terminal handle (raw-mode lifecycle, etc.).
    pub fn terminal_mut(&mut self) -> &mut Terminal<I, O> {
        &mut self.terminal
    }

    /// Borrow the screen.
    pub fn screen(&self) -> &Canvas<O> {
        &self.screen
    }

    /// Mutably borrow the screen (mode setters, manual rendering).
    pub fn screen_mut(&mut self) -> &mut Canvas<O> {
        &mut self.screen
    }

    /// Lock the shared event source for synchronous reading. Returns a
    /// guard that derefs to the [`EventSource`] (so `poll`, `try_read`,
    /// `read` work directly).
    pub fn events(&self) -> MutexGuard<'_, EventSource<I>> {
        self.events.lock().unwrap()
    }

    /// A clone of the shared event source handle, for callers that need
    /// their own (e.g. to read it on a dedicated thread or build an
    /// [`EventStream`](uncurses::event::EventStream)).
    pub fn shared_events(&self) -> Arc<Mutex<EventSource<I>>> {
        Arc::clone(&self.events)
    }
}

impl<I, O> UncursesBackend<I, O>
where
    I: Input,
    O: OutputHandle,
{
    /// Enter raw mode, returning the prior state (also cached for
    /// [`restore`](Self::restore)). Call once before driving the UI.
    pub fn init(&mut self) -> io::Result<State> {
        self.terminal.make_raw()
    }

    /// Restore the terminal: reset the screen (alt-screen, cursor, modes),
    /// flush, then revert raw mode. The single teardown entry point.
    pub fn restore(&mut self) -> io::Result<()> {
        self.screen.reset();
        Write::flush(&mut self.screen)?;
        self.terminal.restore()
    }
}

#[cfg(feature = "async")]
impl<I, O> UncursesBackend<I, O>
where
    I: Input + 'static,
    O: Write,
{
    /// Take an asynchronous [`EventStream`](uncurses::event::EventStream)
    /// over the shared event source. The backend keeps its handle, so
    /// synchronous reads via [`events`](Self::events) continue to work
    /// alongside the stream (see the stream's coexistence caveats).
    pub fn event_stream(&self) -> uncurses::event::EventStream<I> {
        uncurses::event::EventStream::from_shared(self.shared_events())
    }
}

impl<I, O> Write for UncursesBackend<I, O>
where
    I: Input,
    O: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.screen.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.screen)
    }
}

impl<I, O> Backend for UncursesBackend<I, O>
where
    I: Input,
    O: OutputHandle,
{
    type Error = io::Error;

    fn draw<'a, J>(&mut self, content: J) -> io::Result<()>
    where
        J: Iterator<Item = (u16, u16, &'a RtCell)>,
    {
        // Keep the screen buffer in step with what ratatui draws into.
        // For an inline viewport the buffer is only the inline height and
        // ratatui's absolute rows are translated down by the viewport top;
        // otherwise it tracks the full terminal size.
        let (full_w, full_h) = self
            .terminal
            .get_window_size()
            .ok()
            .map(|s| (s.col, s.row))
            .filter(|&(c, r)| c != 0 && r != 0)
            .unwrap_or((self.screen.width(), self.screen.height()));
        let (w, h, top) = match self.viewport {
            Viewport::Inline(height) => {
                let h = height.min(full_h);
                let top = self.inline_origin.min(full_h.saturating_sub(h));
                (full_w, h, top)
            }
            _ => (full_w, full_h, 0),
        };
        // Repaint in full if the terminal size changed since the last
        // observation (covers cases where the buffer dimensions stay the
        // same, e.g. an inline viewport on a vertical-only resize).
        self.note_size((full_w, full_h));
        if self.size_dirty.take() {
            self.screen.invalidate();
        }
        if (w, h) != (self.screen.width(), self.screen.height()) {
            self.screen.invalidate();
            self.screen.resize(w, h);
        }
        for (x, y, rc) in content {
            let cell = cell_from_ratatui(rc);
            self.screen.set_cell((x, y.saturating_sub(top)), &cell);
        }
        self.screen.render();
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.screen.set_cursor_visible(false);
        Write::flush(&mut self.screen)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.screen.set_cursor_visible(true);
        Write::flush(&mut self.screen)
    }

    fn get_cursor_position(&mut self) -> io::Result<RtPosition> {
        // Cursor-position querying (CPR) is not implemented under the
        // single-owner event model: a synchronous request/reply cannot
        // share the input with a reader thread. Report the origin for now;
        // inline-viewport anchoring that relies on this will be revisited.
        Ok(RtPosition { x: 0, y: 0 })
    }

    fn set_cursor_position<P: Into<RtPosition>>(&mut self, position: P) -> io::Result<()> {
        let p = position.into();
        // Remember the requested row: when ratatui clears an inline
        // viewport it places the cursor at the viewport top first, letting
        // `clear_region` recover the (possibly shifted) origin on resize.
        self.last_cursor_row = p.y;
        // Emit an absolute CUP directly rather than going through the
        // renderer's cost-optimized (possibly relative) move: ratatui calls
        // this to place its own cursor, so the move must be unconditional
        // and absolute.
        uncurses::ansi::cursor::write_cup(&mut self.screen, p.y, p.x)?;
        // Keep the renderer's cursor bookkeeping in step with the move we
        // just made, translated into the (inline) buffer. In relative-cursor
        // mode merely invalidating would lose the absolute row — the next
        // frame's vertical moves would drift the viewport — so assert the
        // exact buffer-relative position instead. For a non-inline viewport
        // `inline_origin` is 0, so this is the absolute position unchanged.
        let top = match self.viewport {
            Viewport::Inline(_) => self.inline_origin,
            _ => 0,
        };
        self.screen.assume_cursor_at(p.x, p.y.saturating_sub(top));
        Write::flush(&mut self.screen)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    /// Blank only the cells covered by `clear_type` in the screen's
    /// staging buffer and restore the cursor to its prior position. The
    /// renderer's diff emits whatever is needed to bring the wire in
    /// sync on the next [`Backend::flush`].
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        let w = self.screen.width();
        let h = self.screen.height();
        let cursor = self.screen.cursor_position();
        if w == 0 || h == 0 {
            return Ok(());
        }
        let region = match clear_type {
            ClearType::All => Some(uncurses::layout::Rect::new(0, 0, w, h)),
            ClearType::AfterCursor if matches!(self.viewport, Viewport::Inline(_)) => {
                // Inline-viewport resize/clear path. ratatui homes the
                // cursor to the viewport's (recomputed) top row, then erases
                // to the end of the screen — for our inline buffer that is
                // the whole thing. Adopt the fresh origin (the app cursor
                // reported by `get_cursor_position` may sit anywhere in the
                // viewport, so this is the authoritative top), and blank the
                // entire staging buffer so the upcoming full repaint starts
                // clean: the staging buffer preserves overlapping cells
                // across a grow, which a diff-style painter never overwrites,
                // and would otherwise duplicate the previous frame's right
                // edge.
                self.inline_origin = self.last_cursor_row;
                Some(uncurses::layout::Rect::new(0, 0, w, h))
            }
            ClearType::AfterCursor => {
                if cursor.y < h {
                    let tail_x = cursor.x.min(w);
                    self.screen.fill_rect(
                        uncurses::layout::Rect::new(tail_x, cursor.y, w - tail_x, 1),
                        &CzCell::BLANK,
                    );
                }
                (cursor.y + 1 < h)
                    .then(|| uncurses::layout::Rect::new(0, cursor.y + 1, w, h - cursor.y - 1))
            }
            ClearType::BeforeCursor => {
                if cursor.y > 0 {
                    self.screen.fill_rect(
                        uncurses::layout::Rect::new(0, 0, w, cursor.y),
                        &CzCell::BLANK,
                    );
                }
                (cursor.y < h).then(|| {
                    let head_w = (cursor.x.min(w).saturating_add(1)).min(w);
                    uncurses::layout::Rect::new(0, cursor.y, head_w, 1)
                })
            }
            ClearType::CurrentLine => {
                (cursor.y < h).then(|| uncurses::layout::Rect::new(0, cursor.y, w, 1))
            }
            ClearType::UntilNewLine => (cursor.y < h && cursor.x < w)
                .then(|| uncurses::layout::Rect::new(cursor.x, cursor.y, w - cursor.x, 1)),
        };
        if let Some(region) = region {
            self.screen.fill_rect(region, &CzCell::BLANK);
        }
        self.screen.invalidate_cursor();
        // Push the staged blanks to the wire so the clear takes effect
        // before this call returns, matching the immediate-clear contract.
        self.screen.render();
        Write::flush(&mut self.screen)
    }

    fn size(&self) -> io::Result<RtSize> {
        // The full terminal size: ratatui needs it to anchor inline
        // viewports and to detect resizes. Fall back to the screen's
        // buffer size if the query fails (e.g. output is not a tty).
        let (width, height) = self
            .terminal
            .get_window_size()
            .ok()
            .map(|s| (s.col, s.row))
            .filter(|&(c, r)| c != 0 && r != 0)
            .unwrap_or((self.screen.width(), self.screen.height()));
        self.note_size((width, height));
        Ok(RtSize { width, height })
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        // One query reports both cell and pixel dimensions; fall back to
        // the screen's buffer size for cells if it fails.
        let ws = self.terminal.get_window_size().ok();
        let (width, height) = ws
            .as_ref()
            .map(|w| (w.col, w.row))
            .filter(|&(c, r)| c != 0 && r != 0)
            .unwrap_or_else(|| (self.screen.width(), self.screen.height()));
        self.note_size((width, height));
        Ok(WindowSize {
            columns_rows: RtSize { width, height },
            pixels: RtSize {
                width: ws.as_ref().map(|w| w.xpixel).unwrap_or(0),
                height: ws.as_ref().map(|w| w.ypixel).unwrap_or(0),
            },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.screen)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        for _ in 0..n {
            let _ = writeln!(self.screen);
        }
        Write::flush(&mut self.screen)
    }

    fn scroll_region_up(&mut self, _region: std::ops::Range<u16>, _amount: u16) -> io::Result<()> {
        Ok(())
    }

    fn scroll_region_down(
        &mut self,
        _region: std::ops::Range<u16>,
        _amount: u16,
    ) -> io::Result<()> {
        Ok(())
    }
}

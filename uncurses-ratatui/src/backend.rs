use std::io::{self, Write};
use std::time::{Duration, Instant};

use ratatui::Viewport;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell as RtCell;
use ratatui::layout::{Position as RtPosition, Size as RtSize};
use uncurses::buffer::SurfaceMut;
use uncurses::cell::Cell as CzCell;
use uncurses::event::{Event, Input};
use uncurses::layout::Position;
use uncurses::screen::{Screen, ScreenOptions};
use uncurses::terminal::{Stdin, Stdout, TtyInput, TtyOutput};

use crate::convert::cell_from_ratatui;

/// Platform bound required for an output handle usable by the backend.
///
/// The handle must be writable, cheaply copyable, and expose the platform OS
/// handle used by terminal mode and window-size operations. Process stdio and
/// controlling-terminal output handles satisfy this bound.
///
/// This trait is sealed only by its bounds: any type that implements the listed
/// platform traits implements `OutputHandle` automatically.
#[cfg(unix)]
pub trait OutputHandle: Write + Copy + std::os::fd::AsFd {}
#[cfg(unix)]
impl<T: Write + Copy + std::os::fd::AsFd> OutputHandle for T {}
/// Platform bound required for an output handle usable by the backend.
///
/// The handle must be writable, cheaply copyable, and expose the platform OS
/// handle used by terminal mode and window-size operations. Process stdio and
/// controlling-terminal output handles satisfy this bound.
///
/// This trait is sealed only by its bounds: any type that implements the listed
/// platform traits implements `OutputHandle` automatically.
#[cfg(windows)]
pub trait OutputHandle: Write + Copy + std::os::windows::io::AsHandle {}
#[cfg(windows)]
impl<T: Write + Copy + std::os::windows::io::AsHandle> OutputHandle for T {}

/// How long [`Backend::get_cursor_position`] waits for a cursor-position
/// report before falling back to the origin. The widget library calls it at
/// most once per inline-viewport setup, so a small budget keeps setup
/// responsive on terminals that never answer.
const CURSOR_QUERY_TIMEOUT: Duration = Duration::from_millis(100);

/// Extract a cursor-position report from a reply event. The report is the
/// [`Event::CursorPosition`] variant, but at terminal row 1 the wire form
/// collides with a modified-F3 key and is decoded as an [`Event::Multi`]
/// carrying both; unwrap that case too.
fn cursor_position_report(ev: &Event) -> Option<Position> {
    match ev {
        Event::CursorPosition(pos) => Some(*pos),
        Event::Multi(events) => events.iter().find_map(|e| match e {
            Event::CursorPosition(pos) => Some(*pos),
            _ => None,
        }),
        _ => None,
    }
}

/// Backend implementation that drives rendering, input, and lifecycle through
/// one high-level [`Screen`].
///
/// ## What it wraps
///
/// The wrapped screen owns the terminal handle, the canvas, and the event
/// source. Keeping those pieces behind one backend means frame rendering,
/// cursor movement, clearing, size tracking, raw-mode setup, and input reads all
/// observe the same terminal state.
///
/// ## Rendering
///
/// [`Backend::draw`] converts each concrete buffer cell to an uncurses cell and
/// writes it into the screen's canvas. It then calls [`Screen::render`] to stage
/// the diff bytes. Actual I/O is left to [`Backend::flush`], except for methods
/// whose backend contract is immediate (`hide_cursor`, `show_cursor`,
/// `set_cursor_position`, `clear`, `clear_region`, and line appending).
///
/// ```text
/// ┌─────────────────────┐
/// │ Frame buffer        │
/// └─────────┬───────────┘
///           │ buffer cells
///           ▼
/// ┌─────────────────────┐
/// │ UncursesBackend     │
/// │ draw + conversion   │
/// └─────────┬───────────┘
///           │ Screen::set_cell / render
///           ▼
/// ┌─────────────────────┐
/// │ Screen + Canvas     │
/// │ diff against output │
/// └─────────┬───────────┘
///           │ flush
///           ▼
///       terminal
/// ```
///
/// ## Viewports
///
/// The default viewport is [`Viewport::Fullscreen`]. The init helpers call
/// [`set_viewport`](Self::set_viewport) with the viewport stored in
/// terminal options. Inline viewports keep an absolute origin in
/// `inline_origin`; drawing subtracts that origin so the screen canvas contains
/// only the inline region.
///
/// ## Events
///
/// Use [`poll_event`](Self::poll_event),
/// [`try_read_event`](Self::try_read_event), and [`read_event`](Self::read_event)
/// for synchronous loops. They delegate to the screen, whose event path also
/// observes terminal capability reports. With the `async` feature, borrow the
/// screen with [`screen_mut`](Self::screen_mut) and use its `events()` stream.
///
/// ## Setup
///
/// Construction is inert: it does not enter raw mode, enter the alternate
/// screen, hide the cursor, or choose a non-default viewport. Call
/// [`init`](Self::init) or [`init_with`](Self::init_with) for manual setup, or
/// use the crate-level setup helpers for process stdio. Call
/// [`restore`](Self::restore) when the session ends.
pub struct UncursesBackend<I: Input, O: Write> {
    screen: Screen<I, O>,
    /// The widget-library viewport, set via [`set_viewport`](Self::set_viewport)
    /// (by `init_with_options`). Determines the screen buffer height
    /// (inline height vs full terminal height) and whether `draw` /
    /// `set_cursor_position` translate absolute rows into the inline
    /// region.
    viewport: Viewport,
    /// Top row of an inline viewport. Seeded at initial setup from
    /// [`get_cursor_position`](Backend::get_cursor_position), then kept
    /// exact across resizes by [`clear_region`](Backend::clear_region),
    /// which observes the cursor the widget library parks at the recomputed
    /// viewport top before clearing. Absolute rows are translated down by this when
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
    /// [`set_cursor_position`](Backend::set_cursor_position). When the widget
    /// library recomputes an inline viewport (initial setup and every resize) it
    /// positions the cursor at the viewport's top row before clearing it,
    /// so [`clear_region`](Backend::clear_region) reads this back as the
    /// fresh `inline_origin` — the only place the *true* viewport top is
    /// observable (the cursor reported by `get_cursor_position` sits at the
    /// app's cursor, which need not be the viewport top).
    last_cursor_row: u16,
}

impl UncursesBackend<Stdin, Stdout> {
    /// Build a backend over process standard input and output.
    ///
    /// This constructs a [`Screen`] with `stdin` and `stdout`, then wraps it in
    /// [`UncursesBackend::new`]. It does not enter raw mode, hide the cursor,
    /// enter the alternate screen, or apply screen options.
    ///
    /// ## Returns
    ///
    /// A backend ready for manual setup or for construction of a widget-library
    /// terminal.
    ///
    /// ## Errors
    ///
    /// Returns errors from [`Screen::stdio`], including failures to inspect the
    /// terminal size or initialize the input event source.
    ///
    /// ## Panics
    ///
    /// Does not intentionally panic.
    ///
    /// ## Usage note
    ///
    /// Prefer crate-level setup helpers when process stdio and conventional
    /// setup are sufficient.
    pub fn stdio() -> io::Result<Self> {
        Ok(Self::new(Screen::stdio()?))
    }
}

impl UncursesBackend<TtyInput, TtyOutput> {
    /// Build a backend over the controlling terminal instead of process stdio.
    ///
    /// This opens the platform controlling terminal (`/dev/tty` on Unix,
    /// console handles on Windows), constructs a [`Screen`], and wraps it in
    /// [`UncursesBackend::new`]. It is useful when standard input or output is
    /// redirected but the application still needs an interactive terminal.
    ///
    /// ## Returns
    ///
    /// A backend ready for manual setup or for construction of a widget-library
    /// terminal.
    ///
    /// ## Errors
    ///
    /// Returns errors from opening or initializing the controlling terminal,
    /// sizing the canvas, or creating the input event source.
    ///
    /// ## Panics
    ///
    /// Does not intentionally panic.
    ///
    /// ## Usage note
    ///
    /// Like [`stdio`](UncursesBackend::stdio), this constructor is inert; call
    /// [`init`](UncursesBackend::init) or
    /// [`init_with`](UncursesBackend::init_with) before interactive use.
    pub fn open() -> io::Result<Self> {
        Ok(Self::new(Screen::open()?))
    }
}

impl<I, O> UncursesBackend<I, O>
where
    I: Input,
    O: Write,
{
    /// Build a backend over an existing [`Screen`].
    ///
    /// Use this when the screen has been constructed by the caller, or when the
    /// terminal handles are not process stdio or the controlling terminal. The
    /// backend starts with [`Viewport::Fullscreen`], an inline origin of `0`,
    /// no remembered terminal size, and no dirty-size flag.
    ///
    /// ## Parameters
    ///
    /// * `screen` - the screen facade that will own rendering, input, and
    ///   terminal lifecycle for this backend.
    ///
    /// ## Returns
    ///
    /// A backend wrapping `screen`.
    ///
    /// ## Panics
    ///
    /// Does not panic.
    ///
    /// ## Usage note
    ///
    /// This does not call [`Screen::init`]. Initialize the screen through the
    /// backend or manually before starting an interactive session.
    pub fn new(screen: Screen<I, O>) -> Self {
        Self {
            screen,
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

    /// Record the viewport used by the surrounding terminal.
    ///
    /// The setup helpers call this with the viewport from terminal options. The
    /// backend uses it to size the screen buffer and, for inline viewports,
    /// translate absolute frame rows into the inline canvas region. The default before this method is called is
    /// [`Viewport::Fullscreen`].
    ///
    /// ## Parameters
    ///
    /// * `viewport` - the viewport selected for the terminal.
    ///
    /// ## Panics
    ///
    /// Does not panic.
    ///
    /// ## Usage note
    ///
    /// For [`Viewport::Inline`], the screen buffer is resized immediately to
    /// the requested height clamped to the current terminal height. Fullscreen
    /// and fixed viewports are stored without resizing here; drawing keeps the
    /// screen in step with the current full size.
    pub fn set_viewport(&mut self, viewport: Viewport) {
        if let Viewport::Inline(height) = viewport {
            let size = self.screen.size();
            let h = height.min(size.height);
            self.screen.resize((size.width, h));
        }
        self.viewport = viewport;
    }

    /// Borrow the wrapped [`Screen`] facade.
    ///
    /// Use this for read-only access to screen state such as cached capability
    /// or size information. Rendering and input operations that mutate the
    /// screen require [`screen_mut`](Self::screen_mut).
    ///
    /// ## Returns
    ///
    /// A shared reference to the screen owned by this backend.
    ///
    /// ## Panics
    ///
    /// Does not panic.
    pub fn screen(&self) -> &Screen<I, O> {
        &self.screen
    }

    /// Mutably borrow the wrapped [`Screen`] facade.
    ///
    /// Use this for screen operations not surfaced by the backend: setting
    /// screen modes, using the alternate screen directly, configuring renderer
    /// options, manual rendering, or accessing the async `events()` stream when
    /// the feature is enabled.
    ///
    /// ## Returns
    ///
    /// A mutable reference to the screen owned by this backend.
    ///
    /// ## Panics
    ///
    /// Does not panic.
    ///
    /// ## Usage note
    ///
    /// Avoid mixing manual canvas writes with normal backend drawing unless the
    /// ordering is deliberate; both paths affect the same canvas.
    pub fn screen_mut(&mut self) -> &mut Screen<I, O> {
        &mut self.screen
    }

    /// Poll the wrapped screen's input source.
    ///
    /// This delegates to [`Screen::poll_event`], which drives the underlying
    /// event source for at most `timeout`. It does not remove an event from the
    /// queue; call [`try_read_event`](Self::try_read_event) or
    /// [`read_event`](Self::read_event) after it reports availability.
    ///
    /// ## Parameters
    ///
    /// * `timeout` - `Some(duration)` to wait up to that duration, or `None` to
    ///   use the event source's blocking poll behavior.
    ///
    /// ## Returns
    ///
    /// `Ok(true)` if an event is available, `Ok(false)` if the poll timed out.
    ///
    /// ## Errors
    ///
    /// Returns I/O errors from the input source.
    ///
    /// ## Panics
    ///
    /// Panics if the screen's internal event-source lock is poisoned.
    ///
    /// ## Usage note
    ///
    /// Polling through the backend keeps capability detection and application
    /// input on the same event source.
    pub fn poll_event(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        self.screen.poll_event(timeout)
    }

    /// Try to read the next queued event without blocking.
    ///
    /// This delegates to [`Screen::try_read_event`]. If an event is queued, the
    /// screen observes it for capability-tracking side effects before returning
    /// it to the caller.
    ///
    /// ## Returns
    ///
    /// `Some(event)` when an event was already queued; `None` when reading would
    /// require blocking or additional I/O.
    ///
    /// ## Panics
    ///
    /// Panics if the screen's internal event-source lock is poisoned.
    ///
    /// ## Usage note
    ///
    /// Pair this with [`poll_event`](Self::poll_event) for timeout-based loops.
    pub fn try_read_event(&mut self) -> Option<Event> {
        self.screen.try_read_event()
    }

    /// Block until the next event is available.
    ///
    /// This delegates to [`Screen::read_event`]. The screen observes the event
    /// for capability-tracking side effects before returning it.
    ///
    /// ## Returns
    ///
    /// The next decoded terminal [`Event`].
    ///
    /// ## Errors
    ///
    /// Returns I/O errors from the input source or from applying
    /// discovery-driven screen defaults triggered by capability replies.
    ///
    /// ## Panics
    ///
    /// Panics if the screen's internal event-source lock is poisoned.
    ///
    /// ## Usage note
    ///
    /// Use this for simple blocking event loops. Use the screen's async event
    /// stream instead when the `async` feature is enabled and the application is
    /// already asynchronous.
    pub fn read_event(&mut self) -> io::Result<Event> {
        self.screen.read_event()
    }
}

impl<I, O> UncursesBackend<I, O>
where
    I: Input + Copy,
    O: OutputHandle,
{
    /// Begin an interactive session with default [`ScreenOptions`].
    ///
    /// This delegates to [`Screen::init`]: the screen enters raw mode, applies
    /// always-on defaults, and stages its terminal capability queries. It does
    /// not enter the alternate screen or hide the cursor by itself; the
    /// crate-level setup helpers perform those additional steps.
    ///
    /// ## Returns
    ///
    /// `Ok(())` after raw mode and screen initialization have been staged.
    ///
    /// ## Errors
    ///
    /// Returns errors from raw-mode setup, autoresizing, bracketed paste setup,
    /// or staging capability queries.
    ///
    /// ## Panics
    ///
    /// Does not intentionally panic.
    ///
    /// ## Usage note
    ///
    /// Pair successful manual initialization with [`restore`](Self::restore).
    pub fn init(&mut self) -> io::Result<()> {
        self.screen.init()
    }

    /// Begin an interactive session with explicit [`ScreenOptions`].
    ///
    /// This delegates to [`Screen::init_with`], allowing the caller to choose
    /// bracketed paste, keyboard enhancements, mouse tracking, in-band resize
    /// preference, and pixel-size behavior before capability queries are staged.
    /// It does not enter the alternate screen or hide the cursor by itself.
    ///
    /// ## Parameters
    ///
    /// * `options` - screen defaults to apply during initialization.
    ///
    /// ## Returns
    ///
    /// `Ok(())` after raw mode and screen initialization have been staged.
    ///
    /// ## Errors
    ///
    /// Returns errors from raw-mode setup, autoresizing, always-on mode setup,
    /// or staging capability queries.
    ///
    /// ## Panics
    ///
    /// Does not intentionally panic.
    ///
    /// ## Usage note
    ///
    /// Pair successful manual initialization with [`restore`](Self::restore).
    pub fn init_with(&mut self, options: ScreenOptions) -> io::Result<()> {
        self.screen.init_with(options)
    }

    /// Restore terminal state after a backend-managed session.
    ///
    /// This delegates to [`Screen::pause`]. The screen stops any async event
    /// stream, tears down staged modes, resets canvas-controlled state such as
    /// alternate screen and cursor visibility, flushes pending output, and
    /// restores the terminal mode while keeping the screen available for future
    /// use.
    ///
    /// ## Returns
    ///
    /// `Ok(())` after teardown and terminal-mode restoration complete.
    ///
    /// ## Errors
    ///
    /// Returns errors from mode teardown, flushing, or terminal restoration.
    ///
    /// ## Panics
    ///
    /// Does not intentionally panic.
    ///
    /// ## Usage note
    ///
    /// Treat this as the single teardown entry point for backend-managed setup.
    pub fn restore(&mut self) -> io::Result<()> {
        self.screen.pause()
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
    I: Input + Copy,
    O: OutputHandle,
{
    type Error = io::Error;

    /// Stage a frame's cells into the wrapped screen canvas.
    ///
    /// The iterator supplies absolute buffer coordinates and concrete cells.
    /// Each cell is converted to an uncurses cell, written to the screen, and
    /// then [`Screen::render`] stages the canvas diff. This method does not
    /// flush the staged bytes; the surrounding terminal calls
    /// [`Backend::flush`] when it wants output written.
    ///
    /// ## Parameters
    ///
    /// * `content` - visible frame cells as `(x, y, cell)` triples.
    ///
    /// ## Errors
    ///
    /// This implementation currently performs no fallible I/O and returns
    /// `Ok(())`. Later flushing may surface renderer or output errors.
    ///
    /// ## Usage note
    ///
    /// Inline viewports translate `y` by the stored inline origin. A detected
    /// terminal-size change invalidates the screen so this frame repaints in
    /// full.
    fn draw<'a, J>(&mut self, content: J) -> io::Result<()>
    where
        J: Iterator<Item = (u16, u16, &'a RtCell)>,
    {
        // Keep the screen buffer in step with what the widget library draws into.
        // For an inline viewport the buffer is only the inline height and
        // the widget library's absolute rows are translated down by the viewport top;
        // otherwise it tracks the full terminal size.
        let size = self.screen.size();
        let (full_w, full_h) = self
            .screen
            .get_window_size()
            .ok()
            .map(|s| (s.col, s.row))
            .filter(|&(c, r)| c != 0 && r != 0)
            .unwrap_or((size.width, size.height));
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
        if (w, h) != (size.width, size.height) {
            self.screen.invalidate();
            self.screen.resize((w, h));
        }
        for (x, y, rc) in content {
            let cell = cell_from_ratatui(rc);
            self.screen.set_cell((x, y.saturating_sub(top)), &cell);
        }
        self.screen.render();
        Ok(())
    }

    /// Hide the terminal cursor immediately.
    ///
    /// Delegates to [`Screen::hide_cursor`], which stages cursor visibility on
    /// the canvas and flushes before returning.
    ///
    /// ## Errors
    ///
    /// Returns output errors from flushing the visibility change.
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.screen.hide_cursor()
    }

    /// Show the terminal cursor immediately.
    ///
    /// Delegates to [`Screen::show_cursor`], which stages cursor visibility on
    /// the canvas and flushes before returning.
    ///
    /// ## Errors
    ///
    /// Returns output errors from flushing the visibility change.
    fn show_cursor(&mut self) -> io::Result<()> {
        self.screen.show_cursor()
    }

    /// Query the terminal for its current cursor position.
    ///
    /// This sends a cursor-position request, then polls input until a
    /// [`Event::CursorPosition`] report arrives or the short setup timeout
    /// expires. Non-report events read while waiting are unread back into the
    /// screen in their original order so the application can still consume them.
    /// If no report arrives, the backend returns the origin.
    ///
    /// ## Returns
    ///
    /// The zero-based cursor position reported by the terminal, or `(0, 0)` on
    /// timeout. Inline viewports also seed their initial origin from this row.
    ///
    /// ## Errors
    ///
    /// Returns errors from writing the cursor-position request or polling the
    /// input source.
    ///
    /// ## Usage note
    ///
    /// The reply parser also accepts the multi-event ambiguity that occurs when
    /// the row-1 CPR wire form collides with a modified function key sequence.
    fn get_cursor_position(&mut self) -> io::Result<RtPosition> {
        // Query the terminal for its cursor position (CPR): write the
        // request, then read events until the report arrives or the timeout
        // elapses. The reply is absolute, zero-based, and matches the widget library's
        // coordinate space (the same space `set_cursor_position` writes
        // absolute moves into), so it needs no translation. Fall back to the
        // origin if the terminal does not answer.
        self.screen.request_cursor_position()?;
        let deadline = Instant::now() + CURSOR_QUERY_TIMEOUT;
        // Events read while waiting for the report are not ours to consume;
        // stash them and put them back (in original order) so the app's loop
        // still sees them.
        let mut stash: Vec<Event> = Vec::new();
        let found = loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break None;
            };
            if !self.screen.poll_event(Some(remaining))? {
                break None;
            }
            match self.screen.try_read_event() {
                Some(ev) => match cursor_position_report(&ev) {
                    Some(pos) => break Some(pos),
                    None => stash.push(ev),
                },
                None => continue,
            }
        };
        for ev in stash.into_iter().rev() {
            self.screen.unread_event(ev);
        }
        let pos = found.unwrap_or(Position::new(0, 0));
        // the widget library calls this to anchor an inline viewport at the cursor row,
        // then draws content at absolute rows starting there. Seed the
        // inline origin so the very first `draw` translates those absolute
        // rows into the inline buffer; before this the origin defaulted to 0
        // and a cursor below the top clipped the first frame. The exact top
        // is still re-derived by `clear_region` on later resizes.
        if matches!(self.viewport, Viewport::Inline(_)) {
            self.inline_origin = pos.y;
            self.last_cursor_row = pos.y;
        }
        Ok(RtPosition { x: pos.x, y: pos.y })
    }

    /// Move the terminal cursor to an absolute position immediately.
    ///
    /// The backend writes an absolute CUP escape directly, records the row for
    /// inline-viewport bookkeeping, updates the renderer's tracked cursor
    /// position in buffer-relative coordinates, and flushes.
    ///
    /// ## Parameters
    ///
    /// * `position` - zero-based absolute cursor position requested by the
    ///   surrounding terminal.
    ///
    /// ## Errors
    ///
    /// Returns output errors from writing or flushing the cursor movement.
    ///
    /// ## Usage note
    ///
    /// Direct absolute movement keeps the renderer and inline viewport aligned;
    /// it intentionally bypasses the renderer's cost-optimized relative moves.
    fn set_cursor_position<P: Into<RtPosition>>(&mut self, position: P) -> io::Result<()> {
        let p = position.into();
        // Remember the requested row: when the widget library clears an inline
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
        self.screen
            .assume_cursor_position((p.x, p.y.saturating_sub(top)));
        Write::flush(&mut self.screen)
    }

    /// Clear the entire backend surface immediately.
    ///
    /// This delegates to [`Backend::clear_region`] with [`ClearType::All`].
    ///
    /// ## Errors
    ///
    /// Returns output errors from rendering and flushing the staged blank cells.
    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    /// Clear part of the backend surface immediately.
    ///
    /// The implementation blanks only the cells covered by `clear_type` in the
    /// screen's staging buffer, invalidates tracked cursor state, renders the
    /// diff, and flushes before returning. For inline viewports,
    /// [`ClearType::AfterCursor`] is also the resize/viewport-reanchor path: the
    /// last absolute cursor row becomes the new inline origin and the full
    /// inline buffer is blanked.
    ///
    /// ## Parameters
    ///
    /// * `clear_type` - the clear region requested by the surrounding terminal.
    ///
    /// ## Errors
    ///
    /// Returns output errors from rendering or flushing the clear operation.
    ///
    /// ## Usage note
    ///
    /// Clearing is immediate by backend contract; unlike [`Backend::draw`], this
    /// method does not wait for a later flush call to make output visible.
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        let size = self.screen.size();
        let w = size.width;
        let h = size.height;
        let cursor = self.screen.tracked_cursor_position();
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
        self.screen.invalidate_tracked_cursor();
        // Push the staged blanks to the wire so the clear takes effect
        // before this call returns, matching the immediate-clear contract.
        self.screen.render();
        Write::flush(&mut self.screen)
    }

    /// Return the current terminal size in cells.
    ///
    /// This queries the live window size through the screen. If that query fails
    /// or returns zero dimensions, it falls back to the current screen canvas
    /// size. Size changes are recorded so the next draw can invalidate and
    /// repaint.
    ///
    /// ## Returns
    ///
    /// The full terminal width and height in cells.
    ///
    /// ## Errors
    ///
    /// This implementation falls back on query failure and currently returns
    /// `Ok` with the best available size.
    fn size(&self) -> io::Result<RtSize> {
        // The full terminal size: the widget library needs it to anchor inline
        // viewports and to detect resizes. Fall back to the screen's
        // buffer size if the query fails (e.g. output is not a tty).
        let size = self.screen.size();
        let (width, height) = self
            .screen
            .get_window_size()
            .ok()
            .map(|s| (s.col, s.row))
            .filter(|&(c, r)| c != 0 && r != 0)
            .unwrap_or((size.width, size.height));
        self.note_size((width, height));
        Ok(RtSize { width, height })
    }

    /// Return the current terminal size in cells and pixels.
    ///
    /// This uses the screen's live window-size query when available. Cell
    /// dimensions fall back to the canvas size on failure or zero reports; pixel
    /// dimensions fall back to zero when unavailable. Size changes are recorded
    /// so the next draw can invalidate and repaint.
    ///
    /// ## Returns
    ///
    /// A [`WindowSize`] with `columns_rows` populated from the best available
    /// cell size and `pixels` populated from the query when reported.
    ///
    /// ## Errors
    ///
    /// This implementation falls back on query failure and currently returns
    /// `Ok` with the best available size.
    fn window_size(&mut self) -> io::Result<WindowSize> {
        // One query reports both cell and pixel dimensions; fall back to
        // the screen's buffer size for cells if it fails.
        let size = self.screen.size();
        let ws = self.screen.get_window_size().ok();
        let (width, height) = ws
            .as_ref()
            .map(|w| (w.col, w.row))
            .filter(|&(c, r)| c != 0 && r != 0)
            .unwrap_or((size.width, size.height));
        self.note_size((width, height));
        Ok(WindowSize {
            columns_rows: RtSize { width, height },
            pixels: RtSize {
                width: ws.as_ref().map(|w| w.xpixel).unwrap_or(0),
                height: ws.as_ref().map(|w| w.ypixel).unwrap_or(0),
            },
        })
    }

    /// Flush bytes staged by the screen renderer.
    ///
    /// This delegates to the wrapped screen's [`Write`] implementation. It is
    /// where bytes staged by [`Backend::draw`] are written to the output handle.
    ///
    /// ## Errors
    ///
    /// Returns output errors from the wrapped screen.
    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.screen)
    }

    /// Append blank lines to the underlying output.
    ///
    /// The backend writes `n` newline-terminated blank lines through the screen
    /// and flushes immediately.
    ///
    /// ## Parameters
    ///
    /// * `n` - number of lines to append.
    ///
    /// ## Errors
    ///
    /// Returns output errors from writing or flushing the lines.
    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        for _ in 0..n {
            let _ = writeln!(self.screen);
        }
        Write::flush(&mut self.screen)
    }

    /// Handle a request to scroll a region upward.
    ///
    /// This backend does not use terminal scrolling for region updates; drawing
    /// and canvas diffing repaint the resulting cells instead. The method is a
    /// no-op that satisfies the backend trait.
    ///
    /// ## Parameters
    ///
    /// * `_region` - ignored requested row range.
    /// * `_amount` - ignored scroll amount.
    ///
    /// ## Errors
    ///
    /// This implementation is infallible and returns `Ok(())`.
    fn scroll_region_up(&mut self, _region: std::ops::Range<u16>, _amount: u16) -> io::Result<()> {
        Ok(())
    }

    /// Handle a request to scroll a region downward.
    ///
    /// This backend does not use terminal scrolling for region updates; drawing
    /// and canvas diffing repaint the resulting cells instead. The method is a
    /// no-op that satisfies the backend trait.
    ///
    /// ## Parameters
    ///
    /// * `_region` - ignored requested row range.
    /// * `_amount` - ignored scroll amount.
    ///
    /// ## Errors
    ///
    /// This implementation is infallible and returns `Ok(())`.
    fn scroll_region_down(
        &mut self,
        _region: std::ops::Range<u16>,
        _amount: u16,
    ) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::cursor_position_report;
    use uncurses::event::{Event, Key, KeyCode, KeyModifiers};
    use uncurses::layout::Position;

    #[test]
    fn report_from_plain_cursor_position() {
        let ev = Event::CursorPosition(Position::new(4, 9));
        assert_eq!(cursor_position_report(&ev), Some(Position::new(4, 9)));
    }

    #[test]
    fn report_unwraps_multi_for_row1_f3_ambiguity() {
        // At terminal row 1 the CPR wire form collides with modified-F3, so
        // the decoder emits both inside a Multi; the report is still found.
        let ev = Event::Multi(vec![
            Event::KeyPress(Key::new(KeyCode::F(3), KeyModifiers::empty())),
            Event::CursorPosition(Position::new(2, 0)),
        ]);
        assert_eq!(cursor_position_report(&ev), Some(Position::new(2, 0)));
    }

    #[test]
    fn report_ignores_unrelated_events() {
        let ev = Event::KeyPress(Key::new(KeyCode::Char('x'), KeyModifiers::empty()));
        assert_eq!(cursor_position_report(&ev), None);
        let multi = Event::Multi(vec![Event::FocusIn, Event::FocusOut]);
        assert_eq!(cursor_position_report(&multi), None);
    }
}

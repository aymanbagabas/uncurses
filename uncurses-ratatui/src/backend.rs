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

/// How long [`Backend::get_cursor_position`] waits for a cursor-position
/// report before falling back to the origin. ratatui calls it at most once
/// per inline-viewport setup, so a small budget keeps setup responsive on
/// terminals that never answer.
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

/// A `ratatui` [`Backend`](ratatui::backend::Backend) built on the
/// high-level [`Screen`] facade. The screen owns the whole terminal stack
/// (the [`Terminal`](uncurses::terminal::Terminal) raw-mode lifecycle and
/// window size, the [`Canvas`](uncurses::canvas::Canvas) it renders
/// through, and a shared event source for input); this backend drives it
/// to satisfy ratatui's drawing, cursor, and clearing contracts.
///
/// Side-effecting methods (`hide_cursor`, `show_cursor`,
/// `set_cursor_position`, `clear`, `clear_region`) flush immediately,
/// while `draw` only updates the back buffer and defers I/O to the next
/// `flush`.
///
/// # Events
///
/// Read input synchronously with [`poll_event`](Self::poll_event),
/// [`try_read_event`](Self::try_read_event), and [`read_event`](Self::read_event) (which delegate
/// to the [`Screen`] and run its capability detection). For an async
/// ratatui loop, drive the screen's stream directly via
/// `screen_mut().events()` (the `async` feature).
///
/// # Setup
///
/// Construction is inert: it does not enter raw mode or the alternate
/// screen. Call [`init`](Self::init) to begin a session through the
/// [`Screen`] facade ([`Screen::init`]: raw mode plus the screen's default
/// modes and capability queries), drive the rest through the facade
/// ([`screen_mut`](Self::screen_mut)), then [`restore`](Self::restore) on
/// exit.
pub struct UncursesBackend<I: Input, O: Write> {
    screen: Screen<I, O>,
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
        Ok(Self::new(Screen::stdio()?))
    }
}

impl UncursesBackend<TtyInput, TtyOutput> {
    /// Build a backend over the controlling terminal (`/dev/tty`, or
    /// `CONIN$`/`CONOUT$` on Windows), bypassing redirected stdio.
    pub fn open() -> io::Result<Self> {
        Ok(Self::new(Screen::open()?))
    }
}

impl<I, O> UncursesBackend<I, O>
where
    I: Input,
    O: Write,
{
    /// Build a backend over an existing [`Screen`], so a caller can supply
    /// a screen sized and configured however they like.
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
            let size = self.screen.size();
            let h = height.min(size.height);
            self.screen.resize((size.width, h));
        }
        self.viewport = viewport;
    }

    /// Borrow the [`Screen`] facade.
    pub fn screen(&self) -> &Screen<I, O> {
        &self.screen
    }

    /// Mutably borrow the [`Screen`] facade (mode setters, manual
    /// rendering, the alternate screen, etc.).
    pub fn screen_mut(&mut self) -> &mut Screen<I, O> {
        &mut self.screen
    }

    /// Drive the input source for up to `timeout`, returning whether an
    /// event became available. Delegates to [`Screen::poll_event`].
    pub fn poll_event(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        self.screen.poll_event(timeout)
    }

    /// Take the next queued event without blocking, running the screen's
    /// capability detection as a side effect. Delegates to
    /// [`Screen::try_read_event`].
    pub fn try_read_event(&mut self) -> Option<Event> {
        self.screen.try_read_event()
    }

    /// Block until the next event, running the screen's capability
    /// detection as a side effect. Delegates to [`Screen::read_event`].
    pub fn read_event(&mut self) -> io::Result<Event> {
        self.screen.read_event()
    }
}

impl<I, O> UncursesBackend<I, O>
where
    I: Input + Copy,
    O: OutputHandle,
{
    /// Begin a session through the [`Screen`] facade: enter raw mode and
    /// apply the screen's default setup ([`Screen::init`]), then drive the
    /// UI. Pair with [`restore`](Self::restore) on exit.
    pub fn init(&mut self) -> io::Result<()> {
        self.screen.init()
    }

    /// Begin a session like [`init`](Self::init) but with explicit
    /// [`ScreenOptions`], to control bracketed paste, keyboard
    /// enhancements, mouse tracking, in-band resize, and pixel-size
    /// behavior. Delegates to [`Screen::init_with`].
    pub fn init_with(&mut self, options: ScreenOptions) -> io::Result<()> {
        self.screen.init_with(options)
    }

    /// Hand the terminal back: tear down every mode the screen staged,
    /// reset the canvas (alt-screen, cursor, modes), flush, then revert raw
    /// mode. The same teardown as [`Screen::finish`], but it keeps the
    /// screen so it can be driven through ratatui's `&mut`-based restore.
    /// The single teardown entry point.
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

    fn draw<'a, J>(&mut self, content: J) -> io::Result<()>
    where
        J: Iterator<Item = (u16, u16, &'a RtCell)>,
    {
        // Keep the screen buffer in step with what ratatui draws into.
        // For an inline viewport the buffer is only the inline height and
        // ratatui's absolute rows are translated down by the viewport top;
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

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.screen.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.screen.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<RtPosition> {
        // Query the terminal for its cursor position (CPR): write the
        // request, then read events until the report arrives or the timeout
        // elapses. The reply is absolute, zero-based, and matches ratatui's
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
        // ratatui calls this to anchor an inline viewport at the cursor row,
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
        self.screen
            .assume_cursor_position((p.x, p.y.saturating_sub(top)));
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

    fn size(&self) -> io::Result<RtSize> {
        // The full terminal size: ratatui needs it to anchor inline
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

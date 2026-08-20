//! [`Program`] — the interactive terminal session.
//!
//! `Program<I, O>` is what you build a terminal application on. It owns the
//! things a session needs and a [`Screen`] to render with:
//!
//! - a [`Terminal`] for the raw-mode lifecycle,
//! - an [`EventSource`] for decoded input,
//! - the terminal and input modes (mouse, bracketed paste, focus reporting,
//!   in-band resize, titles, colors, cursor style, keyboard enhancements),
//!   tracked so they can be torn down on a shell handoff and re-applied after,
//! - the [`Capabilities`] the terminal has reported, recorded from replies as
//!   they pass through the read path.
//!
//! Drawing is not on `Program`. Reach the renderer with
//! [`screen_mut`](Program::screen_mut) and call
//! [`render`](Screen::render) on it — that is the only `render` in the crate,
//! and the only `flush`.
//!
//! Construction is inert: [`Program::new`] (and the [`stdio`](Program::stdio)
//! / [`open`](Program::open) shortcuts) only build the program. Begin a
//! session with [`Program::init`], which enters raw mode. Nothing is probed
//! unless you ask: call [`Program::query_capabilities`] for that. Teardown is
//! explicit: there is **no** `Drop`.
//! Hand the terminal back to the shell with [`Program::finish`] (consume),
//! [`Program::pause`] (keep, e.g. to shell out), or [`Program::suspend`]
//! (pause, then stop the process with `SIGTSTP`); resume a
//! paused/suspended program with [`Program::resume`].
//!
//! ```no_run
//! use uncurses::program::Program;
//! use uncurses::style::Style;
//! use uncurses::text::TextSurface;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut program = Program::open()?; // build over /dev/tty
//! program.init()?; // raw mode; probes nothing on its own
//! program.enter_alt_screen()?;
//!
//! let screen = program.screen_mut();
//! screen.set_str((0, 0), "hello", Style::default());
//! screen.render()?;
//!
//! let event = program.read_event()?; // reply tracking is automatic
//! program.finish()?; // restore the terminal
//! # Ok(())
//! # }
//! ```
//!
//! # Options and defaults
//!
//! [`init`](Program::init) uses [`ProgramOptions::default`];
//! [`init_with`](Program::init_with) takes an explicit [`ProgramOptions`] to
//! choose whether to enable bracketed paste and mouse tracking at startup.
//! They take effect immediately; everything else, capability queries
//! included, is yours to ask for (see
//! [`capabilities`](Program::capabilities)).
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

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bitflags::bitflags;

use crate::ansi::{mode, progress};
use crate::color::Profile;
use crate::event::Input;
use crate::event::{Event, EventSource};
use crate::layout::{Position, Size};
use crate::renderer::Optimizations;
use crate::screen::Screen;
use crate::terminal::Terminal;

/// An interactive terminal session composing a [`Terminal`], an
/// [`EventSource`], and a [`Screen`] with the terminal and input modes. See
/// the [module documentation](self) for the lifecycle.
///
/// `Program` is [`Send`] and [`Sync`] whenever its input and output handles
/// are, so it can be moved onto another thread or held across an `.await`
/// point in a multi-threaded async runtime.
///
/// [`Terminal`]: crate::terminal::Terminal
/// [`EventSource`]: crate::event::EventSource
pub struct Program<I, O>
where
    I: Input,
    O: Write,
{
    /// Owns the raw-mode state and answers the fd-bound queries (window size,
    /// is-a-tty). Never written through: output goes through `screen`, which
    /// holds a copy of the same handle, so one staging buffer keeps
    /// everything in order.
    terminal: Terminal<I, O>,
    /// The renderer. Reach it with [`screen`](Self::screen) /
    /// [`screen_mut`](Self::screen_mut).
    screen: Screen<O>,
    /// Input source behind the read path ([`Self::read_event`] and friends).
    /// Held in an `Arc<Mutex<_>>`; the lock is uncontended in the common
    /// single-reader case.
    source: Arc<Mutex<EventSource<I>>>,
    /// Events handed back by [`Self::unread_event`], which the read path
    /// drains before the source. Kept here rather than in `source` because
    /// these were already observed: routing them back through the source
    /// would observe them a second time, and a reply counts once.
    unread: VecDeque<Event>,
    state: state::State,
    /// Terminal capabilities detected by intercepting the replies to the
    /// queries the application fires.
    caps: Capabilities,
    /// Desired default behaviors, set by [`Self::init_with`].
    options: ProgramOptions,
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
    /// Physical screen coordinate (0-based, from the terminal's top-left) of
    /// the managed area's top-left cell, tracked for inline sessions. Only
    /// meaningful inline; fullscreen [`origin`](Self::origin) is always
    /// `(0, 0)`. Refreshed by [`request_origin`](Self::request_origin), whose
    /// reply is captured in [`observe_event`](Self::observe_event).
    origin: Position,
    /// How many origin `CSI 6n` requests are outstanding, so
    /// [`observe_event`](Self::observe_event) knows which
    /// [`CursorPosition`](Event::CursorPosition) replies are ours to capture.
    /// A count rather than a flag so a burst of requests keeps the last
    /// reply instead of the first.
    origin_queries_pending: u16,
}

/// Defaults applied by [`Program::init_with`].
///
/// Every field here takes effect at init unconditionally — nothing in this
/// struct depends on capability detection, because a [`Program`] never probes
/// the terminal on its own. Call
/// [`query_capabilities`](Program::query_capabilities) if you want
/// [`Capabilities`] populated, then act on them yourself.
#[derive(Debug, Clone)]
pub struct ProgramOptions {
    /// Enable bracketed paste at init. Defaults to `true`.
    pub bracketed_paste: bool,
    /// Enable mouse tracking at init with the given [`MouseTracking`] extras
    /// (see [`Program::enable_mouse`]). The request is emitted unconditionally;
    /// terminals ignore modes they do not support and degrade gracefully.
    /// Defaults to `None` (mouse tracking off).
    pub mouse: Option<MouseTracking>,
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
    /// Mouse tracking is turned *off* through [`Program::disable_mouse`] or by
    /// leaving [`ProgramOptions::mouse`] as `None`, not by an empty flag set.
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

/// A progress indication reported to the terminal with `OSC 9;4`, shown in
/// the taskbar, tab, or window chrome by terminals that support it.
///
/// Set it with [`Program::set_progress_state`] and take it down with
/// [`Program::reset_progress_state`]. Percentages are clamped to `0..=100`
/// when emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProgressState {
    /// Determinate progress at the given percentage.
    Normal(u8),
    /// A failed operation, at the given percentage. Usually red.
    Error(u8),
    /// An operation needing attention, at the given percentage. Usually
    /// yellow. Named "paused" in ConEmu, which originated the sequence.
    Warning(u8),
    /// Work in progress of unknown duration. Usually a pulsing bar.
    Indeterminate,
}

impl ProgressState {
    /// Emit the `OSC 9;4` sequence for this state.
    fn write<W: Write>(self, w: &mut W) -> io::Result<()> {
        match self {
            ProgressState::Normal(p) => progress::write_set_progress_bar(w, p.into()),
            ProgressState::Error(p) => progress::write_set_error_progress_bar(w, p.into()),
            ProgressState::Warning(p) => progress::write_set_warning_progress_bar(w, p.into()),
            ProgressState::Indeterminate => w.write_all(progress::SET_INDETERMINATE_PROGRESS_BAR),
        }
    }
}

impl Default for ProgramOptions {
    fn default() -> Self {
        Self {
            bracketed_paste: true,
            mouse: None,
        }
    }
}

impl<I, O> Program<I, O>
where
    I: Input,
    O: Write,
{
    // --- The screen ------------------------------------------------------

    /// Borrow the [`Screen`] this program renders with.
    pub fn screen(&self) -> &Screen<O> {
        &self.screen
    }

    /// Borrow the [`Screen`] mutably — this is how you draw.
    ///
    /// ```no_run
    /// # use uncurses::program::Program;
    /// # use uncurses::style::Style;
    /// # use uncurses::text::TextSurface;
    /// # fn main() -> std::io::Result<()> {
    /// # let mut program = Program::open()?;
    /// let screen = program.screen_mut();
    /// screen.set_str((0, 0), "hello", Style::default());
    /// screen.render()?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Drawing is the expected use. The screen's *render properties* are also
    /// reachable here, and setting one directly moves only how frames are
    /// drawn — it emits no mode, so the terminal never hears about it. Prefer
    /// the program's own [`enter_alt_screen`](Self::enter_alt_screen),
    /// [`hide_cursor`](Self::hide_cursor), and
    /// [`enable_grapheme_clusters`](Self::enable_grapheme_clusters), which
    /// emit the mode *and* move the property together. Teardown follows what
    /// this program emitted, so a property changed behind its back is not
    /// undone by [`finish`](Self::finish) and does not survive a
    /// [`pause`](Self::pause) / [`resume`](Self::resume) round trip.
    pub fn screen_mut(&mut self) -> &mut Screen<O> {
        &mut self.screen
    }

    // --- Event delegates -------------------------------------------------

    /// Drive the input source for up to `timeout`, returning whether any
    /// event became available. See [`EventSource::poll`].
    pub fn poll_event(&self, timeout: Option<Duration>) -> io::Result<bool> {
        if !self.unread.is_empty() {
            return Ok(true);
        }
        let ready = self.source.lock().unwrap().poll(timeout)?;
        Ok(ready)
    }

    /// Take the next queued event without doing I/O, tracking capabilities as
    /// it passes through. See [`EventSource::try_read`].
    pub fn try_read_event(&mut self) -> io::Result<Option<Event>> {
        if let Some(event) = self.unread.pop_front() {
            return Ok(Some(event));
        }
        let Some(event) = self.source.lock().unwrap().try_read() else {
            return Ok(None);
        };
        self.observe_event(&event)?;
        Ok(Some(event))
    }

    /// Block until the next event, tracking capabilities as it passes
    /// through. See [`EventSource::read`].
    pub fn read_event(&mut self) -> io::Result<Event> {
        if let Some(event) = self.unread.pop_front() {
            return Ok(event);
        }
        let event = self.source.lock().unwrap().read()?;
        self.observe_event(&event)?;
        Ok(event)
    }

    /// Return an event to the front of the input queue, so the next
    /// [`read_event`](Self::read_event) / [`try_read_event`](Self::try_read_event)
    /// yields it before anything already queued. Restore a batch in original
    /// order by unreading in reverse.
    ///
    /// The event was observed on its way out and is deliberately not observed
    /// again on the way back in: a reply counts once, and observing it twice
    /// would match it against two of the requests still in flight. These
    /// events are therefore held by the program, not returned to the shared
    /// [`EventSource`] — use [`EventSource::unread`] through
    /// [`event_source`](Self::event_source) for events the program never saw.
    pub fn unread_event(&mut self, event: Event) {
        self.unread.push_front(event);
    }

    /// A shared handle to the input source behind
    /// [`read_event`](Self::read_event) and friends, for driving input from a
    /// separate reader over the same decoder rather than a second one racing
    /// the same file descriptor.
    ///
    /// The main use is async input: build an
    /// [`EventStream`](crate::event::EventStream) with
    /// [`EventStream::from_shared`](crate::event::EventStream::from_shared) from
    /// this handle and poll it on your executor.
    ///
    /// Events taken this way bypass the program, so capability tracking does
    /// not run on them — feed each one to
    /// [`observe_event`](Self::observe_event) yourself.
    ///
    /// Sharing one source between a live reader and the program's own
    /// [`read_event`](Self::read_event) is best-effort: an event goes to
    /// whichever consumer drains it first, so pick one reader in steady state.
    pub fn event_source(&self) -> Arc<Mutex<EventSource<I>>> {
        Arc::clone(&self.source)
    }

    /// Build an async [`EventStream`](crate::event::EventStream) over this
    /// program's input, for reading events with `events.next().await` inside a
    /// `select!` on any executor. The stream shares the program's decoder, so
    /// it does not race a second reader on the same file descriptor.
    ///
    /// The stream hands back events directly, so — unlike
    /// [`read_event`](Self::read_event) — capability tracking does not run.
    /// Pass each event to [`observe_event`](Self::observe_event) to keep it
    /// alive. Read through the stream *or* through `read_event` in steady
    /// state, not both at once: a shared source hands each event to whichever
    /// consumer drains it first.
    #[cfg(feature = "async")]
    pub fn event_stream(&self) -> crate::event::EventStream<I>
    where
        I: 'static,
    {
        crate::event::EventStream::from_shared(Arc::clone(&self.source))
    }

    // --- Capabilities and geometry ---------------------------------------

    /// Terminal capabilities detected so far from intercepted query
    /// replies. Populated as the relevant reports arrive through the event
    /// delegates after [`Self::init`].
    pub fn capabilities(&self) -> Capabilities {
        self.caps
    }

    /// Last observed full terminal size in cells, or `None` before the first
    /// observation. This is the whole terminal, which inline differs from the
    /// managed area returned by [`Screen::size`].
    pub fn window_cells(&self) -> Option<Size> {
        self.window_cells
    }

    /// Last observed full terminal size in pixels, or `None` when the
    /// terminal has not reported one.
    pub fn window_pixels(&self) -> Option<Size> {
        self.window_pixels
    }

    /// The terminal's self-reported name from its XTVERSION reply (e.g.
    /// `"XTerm(380)"`), or `None` when it has not answered.
    pub fn terminal_name(&self) -> Option<&str> {
        self.terminal_name.as_deref()
    }

    /// Convert a mouse event carrying pixel coordinates into cell
    /// coordinates, using the last observed window pixel and cell sizes.
    /// Returns `None` when either is unknown or degenerate. Neither is
    /// refreshed on its own: call
    /// [`request_window_pixel_size`](Self::request_window_pixel_size) at
    /// startup, and again after a resize or a font-size change.
    pub fn mouse_pixels_to_cells(&self, mouse: crate::event::Mouse) -> Option<crate::event::Mouse> {
        let pixels = self.window_pixels?;
        let cells = self.window_cells?;
        if pixels.width == 0 || pixels.height == 0 || cells.width == 0 || cells.height == 0 {
            return None;
        }
        Some(crate::event::mouse_pixel_to_cell(
            mouse,
            pixels.width,
            pixels.height,
            cells.width,
            cells.height,
        ))
    }

    /// The tracked physical screen coordinate of the managed area's top-left
    /// cell. Always `(0, 0)` in fullscreen. Inline it holds whatever the last
    /// [`request_origin`](Self::request_origin) reply reported, and stays at
    /// `(0, 0)` until you make that call.
    pub fn origin(&self) -> Position {
        if self.screen.fullscreen() {
            Position::ORIGIN
        } else {
            self.origin
        }
    }

    /// Translate a mouse event's screen coordinates into coordinates relative
    /// to the managed area, by subtracting the tracked [`origin`](Self::origin).
    /// A no-op in fullscreen, where the origin is `(0, 0)`, and inline until
    /// [`request_origin`](Self::request_origin) has answered.
    pub fn mouse_to_origin(&self, mouse: crate::event::Mouse) -> crate::event::Mouse {
        let origin = self.origin();
        crate::event::Mouse::new(
            mouse.x.saturating_sub(origin.x),
            mouse.y.saturating_sub(origin.y),
            mouse.button,
            mouse.modifiers,
        )
    }

    /// Cache a fresh terminal size. Pure bookkeeping: nothing is written, and
    /// the pixel dimensions are only updated when the size carried them. Some
    /// platforms (the Windows console) report cell sizes only, leaving
    /// [`window_pixels`](Self::window_pixels) at its last known value; call
    /// [`request_window_pixel_size`](Self::request_window_pixel_size) to
    /// refresh it.
    fn cache_window_size(&mut self, ws: crate::terminal::Winsize) {
        self.window_cells = Some(Size::new(ws.col, ws.row));
        if ws.xpixel > 0 && ws.ypixel > 0 {
            self.window_pixels = Some(Size::new(ws.xpixel, ws.ypixel));
        }
    }

    /// Clip a queried origin so the whole managed area stays on screen: when
    /// the area is shorter than the terminal, its top row sits no lower than
    /// `terminal_height - area_height`.
    fn clip_origin(&self, pos: Position) -> Position {
        let height = self.screen.size().height;
        let terminal_height = self.window_cells.map_or(height, |s| s.height);
        let max_y = terminal_height.saturating_sub(height);
        Position::new(pos.x, pos.y.min(max_y))
    }

    /// Apply an event to the program's capability tracking. The event is
    /// inspected, never consumed.
    ///
    /// [`read_event`](Self::read_event) and
    /// [`try_read_event`](Self::try_read_event) call this for you — you only
    /// need it when you take events from somewhere else, namely the async
    /// [`event_stream`](Self::event_stream) or a shared
    /// [`event_source`](Self::event_source).
    ///
    /// Observe each event exactly once. A second call on an event a read
    /// already observed is not harmless: replies are matched against the
    /// requests still in flight, so observing one reply twice consumes two
    /// requests and the answer to the second goes unrecorded.
    ///
    /// Capability-report replies to the queries you fire with
    /// [`query_capabilities`](Self::query_capabilities) and the individual
    /// `request_*` methods are recorded into
    /// [`capabilities`](Self::capabilities), window-size reports update
    /// [`window_cells`](Self::window_cells) /
    /// [`window_pixels`](Self::window_pixels), and the render-affecting reports
    /// are applied to the [`Screen`].
    ///
    /// Observing never queries. Nothing here asks the terminal a question, so
    /// no reply appears on the event stream that the application did not ask
    /// for. Values the terminal only reports on request, such as the pixel
    /// sizes and the inline [`origin`](Self::origin), go stale until you call
    /// the matching `request_*` method.
    ///
    /// ```ignore
    /// // Async loop: the stream bypasses the program, so observe explicitly.
    /// use tokio_stream::StreamExt;
    ///
    /// let mut events = program.event_stream();
    /// while let Some(ev) = events.next().await {
    ///     let ev = ev?;
    ///     program.observe_event(&ev)?;
    ///     // ... handle ev ...
    ///     program.screen_mut().render()?;
    /// }
    /// ```
    pub fn observe_event(&mut self, event: &Event) -> io::Result<()> {
        use crate::ansi::mode::Mode;
        match *event {
            Event::ModeReport { mode, setting } if setting.is_available() => match mode {
                // Render-affecting and free to adopt: the screen emits the
                // 2026 markers per frame, so knowing the terminal understands
                // them is all it takes. Override with
                // [`Screen::set_synchronized_output`].
                Mode::SYNCHRONIZED_OUTPUT => {
                    self.caps.synchronized_output = true;
                    self.screen.set_synchronized_output(true);
                }
                Mode::UNICODE_CORE => self.caps.grapheme_clusters = true,
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
            }
            Event::TerminalName(ref report) => {
                self.terminal_name = Some(report.clone());
            }
            // Cache the full terminal size as it changes. Refitting the
            // managed area is left to the app (call autoresize() as desired).
            Event::Resize(ws) => {
                self.cache_window_size(ws);
            }
            Event::WindowCellSize { width, height } => {
                self.window_cells = Some(Size::new(width, height));
            }
            Event::WindowPixelSize { width, height } => {
                self.window_pixels = Some(Size::new(width, height));
            }
            // Capture the reply to our own `request_origin`. Observing never
            // consumes, so an application that also queries the cursor still
            // sees this event.
            Event::CursorPosition(pos) if self.origin_queries_pending > 0 => {
                self.origin_queries_pending -= 1;
                self.origin = self.clip_origin(pos);
            }
            // A successful XTGETTCAP reply for a truecolor capability
            // confirms direct-color support: record and upgrade the
            // renderer's color profile.
            Event::Termcap {
                recognized: true,
                ref payload,
            } if payload.contains("RGB") || payload.contains("Tc") => {
                self.caps.true_color = true;
                self.screen
                    .set_color_profile(crate::color::Profile::TrueColor);
            }
            _ => {}
        }
        Ok(())
    }

    /// Enable [`TABS`](Optimizations::TABS) and [`BS`](Optimizations::BS),
    /// the two optimizations raw mode makes safe: `\t` and `\x08` now
    /// reach the terminal intact instead of being rewritten on the way.
    ///
    /// On Unix raw mode clears `OPOST`, disabling output processing
    /// wholesale, so the kernel can no longer expand `\t` into spaces. On
    /// Windows it enables virtual-terminal processing, which is the same
    /// bargain. So this reads no terminal state: `make_raw` returning
    /// `Ok` *is* the answer, and no `$TERM` baseline carries either flag,
    /// so a program that never enters raw mode emits escape sequences
    /// instead.
    ///
    /// [`ONLCR`](Optimizations::ONLCR) is deliberately untouched. Raw
    /// mode makes it false, but it is opt-in: a caller who set it knows
    /// something about their output path that we do not, and clobbering
    /// that is worse than the bytes it would save.
    ///
    /// Runs after every successful `make_raw`, including
    /// [`resume`](Self::resume), since whatever ran while paused may have
    /// put the terminal back into cooked mode.
    #[cfg(any(unix, windows))]
    fn enable_tabs_and_bs(&mut self) {
        let opts = self.screen.optimizations().with_tabs(true).with_bs(true);
        self.screen.set_optimizations(opts);
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

    /// Detect the environment-derived color profile and apply it to the
    /// screen, clamping to no color when the output half is not a terminal
    /// (e.g. redirected to a file or pipe). `is_tty` is the output's
    /// terminal status; the caller supplies it since the platform handle
    /// bounds live on `init_with`.
    fn apply_env_color_profile(&mut self, is_tty: bool) {
        let profile = Profile::detect_from(self.terminal.env(), is_tty);
        self.screen.set_color_profile(profile);
    }

    /// Reconcile the terminal's hardware tab stops with the every-eight
    /// columns layout the renderer assumes. A prior program may have left
    /// arbitrary stops behind, which would make the `HT` (`\t`) moves the
    /// cursor planner emits land on the wrong columns. Modern terminals
    /// reset in one cursor-safe write via DECST8C; the rest get the
    /// portable TBC-then-HTS fallback. Staged and flushed so it reaches
    /// the terminal even when capability queries are disabled.
    ///
    /// Runs whether or not `TABS` is set: the stops belong to the
    /// terminal, not to our willingness to use them, so turning `TABS` on
    /// later must not find them unknown.
    fn reset_tab_stops(&mut self) -> io::Result<()> {
        if Optimizations::supports_decst8c(self.terminal.env()) {
            self.screen
                .write_all(crate::ansi::screen::SET_TAB_EVERY_8_COLUMNS)?;
        } else {
            let width = self.screen.size().width;
            crate::ansi::screen::write_reset_tab_stops_every_8(&mut self.screen, width)?;
        }
        self.screen.flush()
    }

    /// Reset LNM (ANSI mode 20) so a `\n` moves the cursor down without
    /// touching the column.
    ///
    /// The cursor planner emits a bare `\n` for downward moves and, unless
    /// [`Optimizations::ONLCR`] says the host's line discipline expands it,
    /// carries the column across unchanged. A terminal left in LNM by a prior
    /// program breaks that assumption on the terminal's side of the wire:
    /// LNM makes a *received* LF return to column 1, so every horizontal leg
    /// planned after a `\n` starts from a column the cursor is not in.
    ///
    /// No query first. LNM reset is the documented default — the VT510
    /// manual asks that it be kept reset — so there is nothing to learn from
    /// asking, and this follows [`reset_tab_stops`](Self::reset_tab_stops) in
    /// imposing the state the planner assumes rather than trusting what it
    /// inherited. Reset on every raw-mode entry, since a program run during a
    /// [`pause`](Self::pause) can set it while we are not looking.
    ///
    /// Staged and flushed so it reaches the terminal even when capability
    /// queries are disabled.
    fn reset_lnm(&mut self) -> io::Result<()> {
        mode::Mode::LINE_FEED_NEW_LINE.reset(&mut self.screen)?;
        self.screen.flush()
    }

    /// Hand the terminal back: reset every staged mode and the managed area to defaults, and flush. The
    /// caller restores the saved raw-mode state afterward.
    fn teardown(&mut self) -> io::Result<()> {
        self.reset()?;
        self.screen.flush()
    }
}

impl<I, O> Program<I, O>
where
    I: Input + Copy,
    O: Write + Copy,
{
    /// Build the screen and event source over `terminal`, sizing the managed
    /// area to `size`. The color profile and renderer optimizations are
    /// detected from the terminal's captured environment. The terminal is
    /// left as-is.
    fn with_render(terminal: Terminal<I, O>, size: (u16, u16)) -> io::Result<Self> {
        let env = terminal.env();
        // Provisional profile; init_with reapplies it with the real
        // output-is-tty signal via apply_env_color_profile.
        let color_profile = Profile::detect_from(env, true);
        let optimizations = Optimizations::from_env(env);
        let mut screen = Screen::new(terminal.output(), size);
        screen.set_color_profile(color_profile);
        screen.set_optimizations(optimizations);

        let source = Arc::new(Mutex::new(EventSource::new(terminal.input())?));
        Ok(Self {
            terminal,
            screen,
            source,
            unread: VecDeque::new(),
            state: state::State::default(),
            caps: Capabilities::default(),
            options: ProgramOptions::default(),
            window_cells: None,
            window_pixels: None,
            terminal_name: None,
            origin: Position::ORIGIN,
            origin_queries_pending: 0,
        })
    }

    /// Probe the terminal for its capabilities, then flush.
    ///
    /// A [`Program`] never queries the terminal on its own — call this when
    /// you want [`capabilities`](Self::capabilities) populated. It writes the
    /// default query set (Kitty keyboard, the DECRQM modes behind
    /// [`Capabilities`], XTVERSION, xterm modifyOtherKeys, and — when the
    /// environment did not already imply true color — XTGETTCAP `RGB`/`Tc`),
    /// then `extra`, then a Primary DA request.
    ///
    /// `extra` is written verbatim, so it can carry any additional query
    /// escapes you want answered under the same Primary DA terminator. Pass
    /// `&[]` for none.
    ///
    /// The DECRQM, XTVERSION, and XTGETTCAP queries are skipped on Apple's
    /// `Terminal.app`, which mishandles them; its known support is recorded
    /// directly instead.
    ///
    /// # Draining the replies is yours
    ///
    /// This method only *writes*. The replies arrive asynchronously as
    /// ordinary events, and reading them is the caller's job — nothing here
    /// waits. Primary DA is sent last precisely so its reply terminates the
    /// stream: read events until [`Event::PrimaryDeviceAttributes`] lands and
    /// every earlier reply has necessarily arrived, at which point
    /// [`capabilities`](Self::capabilities) is complete.
    ///
    /// If you never read that far, the unread replies are still sitting in the
    /// input buffer when you restore the terminal, and the shell will see them
    /// as typed input. A terminal that answers nothing never sends the Primary
    /// DA reply either, so bound the wait yourself with
    /// [`poll_event`](Self::poll_event).
    ///
    /// ```no_run
    /// # use uncurses::{program::Program, event::Event};
    /// # use std::time::{Duration, Instant};
    /// # fn main() -> std::io::Result<()> {
    /// let mut program = Program::stdio()?;
    /// program.init()?;
    /// program.query_capabilities(&[])?;
    ///
    /// let deadline = Instant::now() + Duration::from_millis(300);
    /// while let Some(timeout) = deadline.checked_duration_since(Instant::now()) {
    ///     if !program.poll_event(Some(timeout))? {
    ///         break;
    ///     }
    ///     if matches!(program.try_read_event()?, Some(Event::PrimaryDeviceAttributes(_))) {
    ///         break;
    ///     }
    /// }
    /// let caps = program.capabilities();
    /// # program.finish()
    /// # }
    /// ```
    ///
    /// [`Event::PrimaryDeviceAttributes`]: crate::event::Event::PrimaryDeviceAttributes
    pub fn query_capabilities(&mut self, extra: &[u8]) -> io::Result<()> {
        use crate::ansi::ctrl::{REQUEST_PRIMARY_DA, REQUEST_XTVERSION};
        use crate::ansi::kitty::REQUEST_KITTY_KEYBOARD;
        use crate::ansi::mode::Mode;
        use crate::ansi::termcap::write_xtgettcap;

        // The env-derived profile is already applied by init_with via
        // apply_env_color_profile; read it back to decide whether there is
        // headroom to upgrade via XTGETTCAP.
        let profile = self.screen.color_profile();

        // Always-safe queries.
        self.screen.write_all(REQUEST_KITTY_KEYBOARD)?;

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
                mode.request(&mut self.screen)?;
            }
            self.screen.write_all(REQUEST_XTVERSION)?;
            self.screen
                .write_all(crate::ansi::xterm::QUERY_MODIFY_OTHER_KEYS)?;
            if profile < Profile::TrueColor {
                // One key per query: some terminals only answer the first
                // capability when several are batched in a single request.
                write_xtgettcap(&mut self.screen, &["RGB"])?;
                write_xtgettcap(&mut self.screen, &["Tc"])?;
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
                self.screen.set_color_profile(Profile::TrueColor);
            }
        }

        self.screen.write_all(extra)?;
        self.screen.write_all(REQUEST_PRIMARY_DA)?;
        self.screen.flush()
    }
}

#[cfg(unix)]
impl<I, O> Program<I, O>
where
    I: Input + Copy + std::os::fd::AsFd,
    O: Write + Copy + std::os::fd::AsFd,
{
    /// Construct a program over `terminal` without touching the terminal:
    /// size the screen to it and create an [`EventSource`] on its input
    /// half. The terminal is left as-is; call [`Self::init`] to enter raw
    /// mode and begin a session.
    pub fn new(terminal: Terminal<I, O>) -> io::Result<Self> {
        let ws = terminal.get_window_size()?;
        Self::with_render(terminal, (ws.col, ws.row))
    }

    /// Begin a session with the default [`ProgramOptions`]. See
    /// [`Self::init_with`].
    pub fn init(&mut self) -> io::Result<()> {
        self.init_with(ProgramOptions::default())
    }

    /// Begin a session: enter raw mode and apply the defaults from
    /// `options`. No capability query is sent; discovery is yours to start
    /// with [`Self::query_capabilities`]. Call once after [`Self::new`],
    /// before rendering.
    pub fn init_with(&mut self, options: ProgramOptions) -> io::Result<()> {
        self.options = options;
        self.terminal.make_raw()?;
        self.enable_tabs_and_bs();
        self.reset_lnm()?;
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
        if let Some(tracking) = self.options.mouse {
            self.enable_mouse(tracking)?;
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
    /// terminal size when fullscreen, or the terminal width with the current
    /// managed height preserved when inline. Refreshes the cached
    /// [`window_cells`](Self::window_cells), and
    /// [`window_pixels`](Self::window_pixels) when the platform reports pixel
    /// dimensions. Nothing is asked of the terminal: this reads the size the
    /// operating system already knows. On platforms whose size query carries
    /// no pixel dimensions (the Windows console), refresh those with
    /// [`request_window_pixel_size`](Self::request_window_pixel_size).
    pub fn autoresize(&mut self) -> io::Result<()> {
        let Ok(ws) = self.terminal.get_window_size() else {
            // Keep the current size when the query fails rather than
            // collapsing the managed area to zero.
            return Ok(());
        };
        self.cache_window_size(ws);
        let height = match self.screen.fullscreen() {
            true => ws.row,
            false => self.screen.size().height,
        };
        self.screen.resize((ws.col, height));
        Ok(())
    }

    /// Consume the program and hand the terminal back to the shell: tear down
    /// every staged mode, reset the managed area, flush, and restore the
    /// terminal's prior state.
    ///
    /// The terminal state is restored even when the teardown writes fail, so a
    /// broken pipe cannot leave the terminal in raw mode. The teardown error is
    /// still returned.
    pub fn finish(mut self) -> io::Result<()> {
        // Restore even when teardown fails. Teardown writes to the output
        // half, so a broken pipe or a closed terminal fails it routinely, and
        // returning early there would leave the terminal raw. `finish` consumes
        // the program, so there would be nothing left to retry with.
        let teardown = self.teardown();
        let restore = self.terminal.restore();
        teardown.and(restore)
    }

    /// Hand the terminal back to the shell without consuming the program,
    /// e.g. to run a child process. Re-enter with [`Self::resume`]. Like
    /// [`Self::finish`] but keeps the program so the session can continue, and
    /// likewise restores the terminal even when the teardown writes fail.
    pub fn pause(&mut self) -> io::Result<()> {
        // Restore even when teardown fails, for the same reason as `finish`:
        // the caller asked for the terminal back, and a failed write to it is
        // not a reason to keep it raw.
        let teardown = self.teardown();
        let restore = self.terminal.restore();
        teardown.and(restore)
    }

    /// Re-acquire the terminal after a [`Self::pause`] or [`Self::suspend`]:
    /// re-enter raw mode, refit the managed area to the current viewport, re-apply
    /// the saved render state and modes, and force a full repaint.
    ///
    /// Re-enables [`TABS`](Optimizations::TABS) and
    /// [`BS`](Optimizations::BS) and resets the hardware tab stops, since
    /// whatever ran while paused may have disturbed both.
    pub fn resume(&mut self) -> io::Result<()> {
        self.terminal.make_raw()?;
        self.enable_tabs_and_bs();
        self.reset_lnm()?;
        self.autoresize()?;
        self.reset_tab_stops()?;
        self.restore()?;
        self.screen.invalidate();
        self.screen.flush()
    }

    /// Suspend the process: [`pause`](Self::pause) the program, then stop
    /// the process with `SIGTSTP`. Returns once the process is
    /// foregrounded again; the caller should then call [`Self::resume`].
    pub fn suspend(&mut self) -> io::Result<()> {
        self.pause()?;
        // SAFETY: raise is async-signal-safe.
        unsafe { libc::raise(libc::SIGTSTP) };
        Ok(())
    }
}

#[cfg(windows)]
impl<I, O> Program<I, O>
where
    I: Input + Copy + std::os::windows::io::AsHandle,
    O: Write + Copy + std::os::windows::io::AsHandle,
{
    /// Construct a program over `terminal` without touching the terminal:
    /// size the screen to it and create an [`EventSource`] on its input
    /// half. The terminal is left as-is; call [`Self::init`] to enter raw
    /// mode and begin a session.
    pub fn new(terminal: Terminal<I, O>) -> io::Result<Self> {
        let ws = terminal.get_window_size()?;
        Self::with_render(terminal, (ws.col, ws.row))
    }

    /// Begin a session with the default [`ProgramOptions`]. See
    /// [`Self::init_with`].
    pub fn init(&mut self) -> io::Result<()> {
        self.init_with(ProgramOptions::default())
    }

    /// Begin a session: enter raw mode and apply the defaults from
    /// `options`. No capability query is sent; discovery is yours to start
    /// with [`Self::query_capabilities`]. Call once after [`Self::new`],
    /// before rendering.
    pub fn init_with(&mut self, options: ProgramOptions) -> io::Result<()> {
        self.options = options;
        self.terminal.make_raw()?;
        self.enable_tabs_and_bs();
        self.reset_lnm()?;
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
        if let Some(tracking) = self.options.mouse {
            self.enable_mouse(tracking)?;
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
    /// terminal size when fullscreen, or the terminal width with the current
    /// managed height preserved when inline. Refreshes the cached
    /// [`window_cells`](Self::window_cells), and
    /// [`window_pixels`](Self::window_pixels) when the platform reports pixel
    /// dimensions. Nothing is asked of the terminal: this reads the size the
    /// operating system already knows. On platforms whose size query carries
    /// no pixel dimensions (the Windows console), refresh those with
    /// [`request_window_pixel_size`](Self::request_window_pixel_size).
    pub fn autoresize(&mut self) -> io::Result<()> {
        let Ok(ws) = self.terminal.get_window_size() else {
            // Keep the current size when the query fails rather than
            // collapsing the managed area to zero.
            return Ok(());
        };
        self.cache_window_size(ws);
        let height = match self.screen.fullscreen() {
            true => ws.row,
            false => self.screen.size().height,
        };
        self.screen.resize((ws.col, height));
        Ok(())
    }

    /// Consume the program and hand the terminal back to the shell: tear down
    /// every staged mode, reset the managed area, flush, and restore the
    /// terminal's prior state.
    ///
    /// The terminal state is restored even when the teardown writes fail, so a
    /// broken pipe cannot leave the terminal in raw mode. The teardown error is
    /// still returned.
    pub fn finish(mut self) -> io::Result<()> {
        let teardown = self.teardown();
        let restore = self.terminal.restore();
        teardown.and(restore)
    }

    /// Hand the terminal back to the shell without consuming the program,
    /// e.g. to run a child process. Re-enter with [`Self::resume`].
    pub fn pause(&mut self) -> io::Result<()> {
        let teardown = self.teardown();
        let restore = self.terminal.restore();
        teardown.and(restore)
    }

    /// Re-acquire the terminal after a [`Self::pause`]: re-enter raw mode,
    /// refit the managed area to the current viewport, re-apply the saved
    /// render state and modes, and force a full repaint.
    ///
    /// Re-enables [`TABS`](Optimizations::TABS) and
    /// [`BS`](Optimizations::BS) and resets the hardware tab stops, since
    /// whatever ran while paused may have disturbed both.
    pub fn resume(&mut self) -> io::Result<()> {
        self.terminal.make_raw()?;
        self.enable_tabs_and_bs();
        self.reset_lnm()?;
        self.autoresize()?;
        self.reset_tab_stops()?;
        self.restore()?;
        self.screen.invalidate();
        self.screen.flush()
    }
}

impl Program<crate::terminal::Stdin, crate::terminal::Stdout> {
    /// Build a program over the process stdio (`stdin` + `stdout`).
    pub fn stdio() -> io::Result<Self> {
        Self::new(Terminal::stdio())
    }
}

impl Program<crate::terminal::TtyInput, crate::terminal::TtyOutput> {
    /// Build a program over the controlling terminal (`/dev/tty`, or
    /// `CONIN$`/`CONOUT$` on Windows), useful when stdio is redirected.
    pub fn open() -> io::Result<Self> {
        Self::new(Terminal::open()?)
    }
}

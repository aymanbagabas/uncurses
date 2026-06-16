//! Unified, wakeable [`EventSource`].
//!
//! Owns the input handle, decodes raw bytes into [`Event`]s with the
//! [`Decoder`], and serves them through [`EventSource::try_read`]
//! which blocks up to a caller-supplied timeout. Each source is paired
//! with a cheaply-clonable [`Waker`] that interrupts an in-progress
//! `try_read` from another thread.
//!
//! Backend per target:
//!
//! * Unix uses the readiness primitive selected in [`super::poll`]
//!   (`epoll` on Linux, `kqueue` on the BSDs, with a `select` fallback
//!   on macOS tty input, and `poll(2)` everywhere else), driving the
//!   input fd, a self-pipe for the [`Waker`], and a SIGWINCH self-pipe
//!   for [`Event::Resize`].
//! * Windows uses `WaitForMultipleObjects` over the console input
//!   handle and a Win32 event used as the cancellation slot; resize
//!   events arrive as `WINDOW_BUFFER_SIZE_EVENT` records on the input
//!   handle.
//!
//! The source tightens its effective wait based on the parser's ESC
//! deadline so a buffered partial escape sequence resolves promptly
//! even when the caller-supplied timeout is longer.

use std::collections::VecDeque;
use std::io;
use std::io::Read;
#[cfg(any(unix, windows))]
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(unix)]
use std::os::fd::OwnedFd;
#[cfg(windows)]
use std::os::windows::io::AsHandle;

use super::decode::{Decoder, is_c1_introducer};
use super::pending::Pending;
#[cfg(any(unix, windows))]
use super::poll::Poller;
#[cfg(unix)]
use super::sigwinch as winch;
#[cfg(unix)]
use super::source_unix::UnixWakerInner;
#[cfg(windows)]
use super::source_windows::WindowsWakerInner;
use crate::event::Event;
#[cfg(unix)]
use crate::terminal::size::Winsize;

/// Bound on what the source needs from an input handle.
///
/// On Unix the source must read bytes (`Read`) and watch the handle for
/// readiness (`AsFd`). On Windows it must read bytes and expose a
/// `HANDLE` for the console-input path (`AsHandle`). A blanket impl is
/// provided for any type that already satisfies the platform bounds.
#[cfg(unix)]
pub trait Input: Read + AsFd + Send {}
#[cfg(unix)]
impl<T: Read + AsFd + Send> Input for T {}

#[cfg(windows)]
pub trait Input: Read + AsHandle + Send {}
#[cfg(windows)]
impl<T: Read + AsHandle + Send> Input for T {}

/// Default read buffer capacity. This is the hard cap on any single
/// sequence; the backing buffer is allocated once at construction.
pub(super) const DEFAULT_BUFFER_CAPACITY: usize = 4096;

/// Default escape-sequence timeout.
///
/// When the read buffer holds a partial `ESC`-prefixed sequence and no
/// further bytes arrive within this window, the source resolves the
/// buffered bytes as a best-effort event (typically a bare Escape
/// keypress).
pub const DEFAULT_ESC_TIMEOUT: Duration = Duration::from_millis(50);

/// Default bracketed-paste idle timeout.
///
/// When a paste has been opened (a `PasteStart` was emitted) and no
/// further input arrives within this window, the source synthesises a
/// `PasteEnd`, flushes any held-back bytes as a final `PasteChunk`,
/// and clears the decoder's paste state. Guards against terminators
/// that never arrive (truncated stream, malformed input).
pub const DEFAULT_PASTE_IDLE_TIMEOUT: Duration = Duration::from_secs(2);

/// Cloneable handle that interrupts an in-progress [`EventSource::try_read`].
///
/// `Send + Sync`; multiple wakes coalesce.
#[derive(Clone)]
pub struct Waker {
    #[cfg(unix)]
    inner: Arc<UnixWakerInner>,
    #[cfg(windows)]
    inner: Arc<WindowsWakerInner>,
    #[cfg(not(any(unix, windows)))]
    _phantom: std::marker::PhantomData<()>,
}

impl Waker {
    #[cfg(unix)]
    pub(super) fn from_unix_inner(inner: Arc<UnixWakerInner>) -> Self {
        Self { inner }
    }

    #[cfg(windows)]
    pub(super) fn from_windows_inner(inner: Arc<WindowsWakerInner>) -> Self {
        Self { inner }
    }

    /// Interrupt the [`EventSource`] this waker is bound to.
    pub fn wake(&self) -> io::Result<()> {
        #[cfg(any(unix, windows))]
        {
            self.inner.wake()
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "waker not supported on this platform",
            ))
        }
    }
}

/// Wakeable event source backed by a platform readiness primitive.
///
/// Single public type across platforms; per-target state and methods
/// are cfg-gated. Construct with [`EventSource::new`] and drive with
/// [`EventSource::try_read`].
pub struct EventSource<I>
where
    I: Input,
{
    /// Owned input handle. Used both as the byte source (`Read`) and,
    /// on Unix, as the readiness target (its fd is registered with the
    /// [`super::poll::Poller`]).
    #[cfg_attr(windows, allow(dead_code))]
    pub(super) input: I,

    pub(super) parser: Decoder,
    /// Bounded read buffer. The unread slice is `pending.slice()`; new
    /// input is written into `pending.spare_mut()`. `pending.capacity()`
    /// is the hard cap on any single sequence and is never resized.
    pub(super) pending: Pending,
    pub(super) esc_timeout: Duration,
    /// Wall-clock instant at which a buffered partial escape sequence
    /// should be force-resolved (typically as a bare `Esc` keypress).
    pub(super) esc_deadline: Option<Instant>,
    /// Idle timeout applied while the decoder is in a bracketed paste.
    /// `None` disables the safety net.
    pub(super) paste_idle_timeout: Option<Duration>,
    /// Wall-clock instant at which an open paste should be
    /// force-closed (synthesised `PasteEnd`).
    pub(super) paste_deadline: Option<Instant>,
    pub(super) queue: VecDeque<Event>,
    pub(super) waker: Waker,
    /// Whether the source delivers [`Event::Resize`] from the
    /// out-of-band kernel resize notification (`SIGWINCH` on Unix).
    /// Defaults to `true`. Set to `false` when in-band resize reports
    /// (DEC mode 2048) are enabled so resizes arrive solely through the
    /// decoder and are not duplicated. No effect on Windows, where resize
    /// is always delivered in-band through the decoder.
    pub(super) handle_resize: bool,

    // --- Unix-only state ---
    /// Shared readiness poller watching `[input, pipe_rx, winch_rx]` (in
    /// that index order). Held behind `Arc` so an [`super::EventStream`]
    /// reader thread can wait on it lock-free while the source's decode
    /// state is mutated under a separate lock.
    #[cfg(unix)]
    pub(super) poller: Arc<dyn Poller>,
    #[cfg(unix)]
    pub(super) pipe_rx: OwnedFd,
    #[cfg(unix)]
    pub(super) winch_rx: OwnedFd,
    /// Held to keep the SIGWINCH write fd alive for the handler.
    /// Dropped after `_winch_sub` so the subscription is removed
    /// before the handler can write into a closed fd.
    #[cfg(unix)]
    pub(super) _winch_tx: OwnedFd,
    /// Active SIGWINCH subscription. Dropped before `_winch_tx` per
    /// declaration order so the handler is unregistered first.
    #[cfg(unix)]
    pub(super) _winch_sub: winch::Subscription,
    #[cfg(unix)]
    pub(super) last_size: Option<Winsize>,

    // --- Windows-only state ---
    /// Shared readiness poller watching `[input_handle, wake_event]` (in
    /// that index order). Held behind `Arc` so an [`super::EventStream`]
    /// reader thread can wait on it lock-free while the source's decode
    /// state is mutated under a separate lock.
    #[cfg(windows)]
    pub(super) poller: Arc<dyn Poller>,
    #[cfg(windows)]
    pub(super) wake_event: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    pub(super) vt_input: bool,
    /// Pending high surrogate per key direction (0 = up, 1 = down).
    /// VT input delivers astral code points as two consecutive
    /// `KEY_EVENT` records, one per UTF-16 unit.
    #[cfg(windows)]
    pub(super) pending_high_surrogate: [Option<u16>; 2],
    #[cfg(windows)]
    pub(super) last_size: Option<(i16, i16)>,
    #[cfg(windows)]
    pub(super) last_mouse_buttons: u32,
}

// Drop ordering note for Unix: Rust drops fields top-to-bottom, so
// _winch_sub (declared before _winch_tx in the struct) is dropped
// first, removing the SIGWINCH handler before the write end of the
// pipe is closed.

/// Which timer governs the next wait. Bookkeeping for `pump` so a
/// `Timeout` outcome can route to the correct expiry handler.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DeadlineKind {
    /// Wait bounded by the caller's `timeout` (or no internal deadline).
    None,
    /// Wait bounded by `esc_deadline` — force a bare `Esc` on expiry.
    Esc,
    /// Wait bounded by `paste_deadline` — synthesise `PasteEnd` on expiry.
    Paste,
}

// ---------------------------------------------------------------------------
// Shared methods (platform-agnostic)
// ---------------------------------------------------------------------------

impl<I> EventSource<I>
where
    I: Input,
{
    /// Configured escape-sequence timeout.
    pub fn esc_timeout(&self) -> Duration {
        self.esc_timeout
    }

    /// Set the escape-sequence timeout — how long to wait for a
    /// continuation byte before treating a buffered partial sequence as
    /// a bare `Esc` keypress. Defaults to [`DEFAULT_ESC_TIMEOUT`].
    ///
    /// Consuming builder; chain after [`EventSource::new`].
    pub fn with_esc_timeout(mut self, timeout: Duration) -> Self {
        self.esc_timeout = timeout;
        self
    }

    /// Set the idle timeout for a bracketed paste with no closing
    /// terminator. `None` disables it, so the source waits indefinitely
    /// for a real `PasteEnd`. Defaults to `Some(`[`DEFAULT_PASTE_IDLE_TIMEOUT`]`)`.
    ///
    /// Consuming builder; chain after [`EventSource::new`].
    pub fn with_paste_idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.paste_idle_timeout = timeout;
        self
    }

    /// Cloneable [`Waker`] bound to this source.
    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }

    /// Whether the source delivers [`Event::Resize`] from the kernel's
    /// out-of-band window-resize notification (`SIGWINCH` on Unix).
    pub fn handle_resize(&self) -> bool {
        self.handle_resize
    }

    /// Control whether the source delivers [`Event::Resize`] from the
    /// kernel's out-of-band window-resize notification (`SIGWINCH` on
    /// Unix). Defaults to `true`.
    ///
    /// Set this to `false` after enabling in-band resize reports (DEC
    /// mode 2048, e.g. via [`Screen::set_in_band_resize`]): the terminal
    /// then reports size changes in-band as `CSI 48 t`, which the decoder
    /// surfaces as [`Event::Resize`], so leaving the `SIGWINCH` path on
    /// would deliver each resize twice. Restore it to `true` when in-band
    /// reporting is disabled again.
    ///
    /// No effect on Windows, where resize is always delivered in-band
    /// through the decoder.
    ///
    /// [`Screen::set_in_band_resize`]: crate::screen::Screen::set_in_band_resize
    pub fn set_handle_resize(&mut self, enable: bool) {
        self.handle_resize = enable;
    }

    /// Shared readiness poller for this source. Cloning the `Arc` lets a
    /// reader thread wait on readiness without holding a lock on the
    /// source's decode state (see [`super::EventStream`]).
    #[cfg(all(feature = "async", any(unix, windows)))]
    pub(super) fn poller(&self) -> Arc<dyn Poller> {
        self.poller.clone()
    }

    /// Number of fds the [`poller`](Self::poller) watches, i.e. the length
    /// of the readiness slice [`Poller::poll`] expects.
    #[cfg(all(feature = "async", unix))]
    pub(super) fn poll_slot_count(&self) -> usize {
        3
    }
    /// Number of handles the [`poller`](Self::poller) watches.
    #[cfg(all(feature = "async", windows))]
    pub(super) fn poll_slot_count(&self) -> usize {
        2
    }

    /// Effective wait for the next readiness poll, plus which deadline (if
    /// any) governs it, so a timeout can be routed to the right expiry.
    #[cfg(all(feature = "async", any(unix, windows)))]
    pub(super) fn next_timeout(&self) -> (Option<Duration>, DeadlineKind) {
        self.effective_timeout(None)
    }

    /// Run the expiry a readiness wait timed out on.
    #[cfg(all(feature = "async", any(unix, windows)))]
    pub(super) fn expire(&mut self, kind: DeadlineKind) {
        match kind {
            DeadlineKind::Esc => self.expire_partial(),
            DeadlineKind::Paste => self.expire_paste(),
            DeadlineKind::None => {}
        }
    }

    /// Whether at least one decoded event is queued.
    #[cfg(all(feature = "async", any(unix, windows)))]
    pub(super) fn has_events(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Block up to `timeout` for at least one event to become available.
    ///
    /// * `Ok(true)` — the event queue has at least one event; [`Self::try_read`]
    ///   or [`Self::read`] will return without blocking.
    /// * `Ok(false)` — `timeout` elapsed without an event, or a paired
    ///   [`Waker`] interrupted the wait.
    /// * `Err(_)` — fatal I/O error.
    ///
    /// `None` for `timeout` means "block until an event or wake".
    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        if !self.queue.is_empty() {
            return Ok(true);
        }
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let remaining = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            match self.pump(remaining) {
                Ok(()) => {
                    if !self.queue.is_empty() {
                        return Ok(true);
                    }
                    if let Some(left) = remaining
                        && left.is_zero()
                    {
                        return Ok(false);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => return Ok(false),
                Err(e) => return Err(e),
            }
        }
    }

    /// Return the next queued event, or `None` if the queue is empty.
    /// Does not perform I/O.
    pub fn try_read(&mut self) -> Option<Event> {
        self.queue.pop_front()
    }

    /// Block until the next event is available, then return it.
    ///
    /// Returns [`io::ErrorKind::Interrupted`] if a paired [`Waker`] fired
    /// while waiting.
    pub fn read(&mut self) -> io::Result<Event> {
        loop {
            if let Some(ev) = self.queue.pop_front() {
                return Ok(ev);
            }
            if !self.poll(None)? {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "wake"));
            }
        }
    }

    /// Remove and return the first queued event satisfying `predicate`,
    /// leaving every other event in place and in order. Returns `None`
    /// if no queued event matches. Does not perform I/O.
    ///
    /// The non-blocking counterpart to [`read_matching`](Self::read_matching),
    /// used to pluck a specific reply (e.g. a query response) out of the
    /// stream without disturbing pending user input.
    pub fn try_read_matching(&mut self, predicate: impl Fn(&Event) -> bool) -> Option<Event> {
        let pos = self.queue.iter().position(predicate)?;
        self.queue.remove(pos)
    }

    /// Block up to `timeout` for an event satisfying `predicate`, remove
    /// it from the queue, and return it. Events that do not match are left
    /// queued, in order, for a later [`read`](Self::read).
    ///
    /// Returns `Ok(None)` on timeout or a paired [`Waker`] wake. `None`
    /// for `timeout` blocks until a match arrives or a wake fires.
    pub fn read_matching(
        &mut self,
        predicate: impl Fn(&Event) -> bool,
        timeout: Option<Duration>,
    ) -> io::Result<Option<Event>> {
        if let Some(ev) = self.try_read_matching(&predicate) {
            return Ok(Some(ev));
        }
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let remaining = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            if !self.poll(remaining)? {
                // Timeout elapsed or a waker fired; nothing new arrived.
                return Ok(None);
            }
            if let Some(ev) = self.try_read_matching(&predicate) {
                return Ok(Some(ev));
            }
            if let Some(left) = remaining
                && left.is_zero()
            {
                return Ok(None);
            }
        }
    }

    /// Drive the parser as far as it will go against the bytes
    /// currently in `pending`, pushing extracted events onto the
    /// queue and arming the appropriate timeout deadline.
    ///
    /// While the decoder is in bracketed paste, the paste-idle
    /// deadline governs (and is reset on every drain, since drain is
    /// only called after fresh input arrived). Otherwise the ESC
    /// disambiguation deadline arms when a partial sequence sits at
    /// the head of `pending`.
    pub(super) fn drain_parser(&mut self) {
        loop {
            let (n, ev) = self.parser.parse_one(self.pending.slice());
            if n == 0 && ev.is_none() {
                break;
            }
            if n > 0 {
                self.pending.consume(n);
            }
            if let Some(ev) = ev {
                self.queue.push_back(ev);
            }
        }

        if self.parser.in_paste() {
            // In paste: only the paste-idle timer applies. Reset on
            // every drain (input has just arrived).
            self.esc_deadline = None;
            self.paste_deadline = self.paste_idle_timeout.map(|t| Instant::now() + t);
            return;
        }

        // Not in paste: clear paste deadline; arm esc deadline if a
        // partial sequence sits at the head.
        self.paste_deadline = None;
        let Some(b0) = self.pending.first() else {
            self.esc_deadline = None;
            return;
        };
        let armable = b0 == 0x1b || is_c1_introducer(b0);
        if armable {
            if self.esc_deadline.is_none() {
                self.esc_deadline = Some(Instant::now() + self.esc_timeout);
            }
        } else {
            self.esc_deadline = None;
        }
    }

    /// Force-resolve a buffered partial sequence whose ESC deadline
    /// elapsed.
    pub(super) fn expire_partial(&mut self) {
        self.esc_deadline = None;
        if self.pending.first().is_none() {
            return;
        }
        // Flip the decoder into expired mode so its recursive ESC
        // handler can commit buffered partial sequences (e.g. resolving
        // `\x1b\x1b` to `Alt+Esc`). Anything the decoder still can't
        // consume falls back to the single-byte fallback below.
        self.parser.set_expired(true);
        self.drain_parser();
        while let Some(b0) = self.pending.first() {
            let ev = self
                .parser
                .expire_leading(b0)
                .unwrap_or_else(|| Event::Unknown(vec![b0]));
            self.pending.consume(1);
            self.queue.push_back(ev);
            self.drain_parser();
        }
        self.parser.set_expired(false);
    }

    /// Force-close a stuck bracketed paste. Flushes any leftover
    /// pending bytes as a final `PasteChunk`, then enqueues
    /// `PasteEnd` and clears the decoder's paste state.
    ///
    /// Called from `pump` when the paste-idle deadline elapses, and
    /// from the public [`EventSource::end_paste`] escape hatch.
    pub(super) fn expire_paste(&mut self) {
        self.paste_deadline = None;
        if !self.parser.in_paste() {
            return;
        }
        if !self.pending.is_empty() {
            let bytes = self.pending.slice().to_vec();
            self.pending.clear();
            self.queue.push_back(Event::PasteChunk(bytes));
        }
        if let Some(ev) = self.parser.end_paste() {
            self.queue.push_back(ev);
        }
    }

    /// Public escape hatch: force-exit bracketed paste mode. Useful
    /// when the embedding application detects misuse (size cap, user
    /// cancel, watchdog) and wants to recover the input stream.
    ///
    /// Has no effect when the decoder is not in paste.
    pub fn end_paste(&mut self) {
        self.expire_paste();
    }

    /// Compute the effective wait, bounded by either the caller
    /// timeout, the ESC deadline, or the paste-idle deadline
    /// (whichever is shorter), and report which one won so a
    /// timeout-return can be routed to the right expiry handler.
    pub(super) fn effective_timeout(
        &self,
        timeout: Option<Duration>,
    ) -> (Option<Duration>, DeadlineKind) {
        let now = Instant::now();
        let esc_remaining = self
            .esc_deadline
            .map(|d| (d.saturating_duration_since(now), DeadlineKind::Esc));
        let paste_remaining = self
            .paste_deadline
            .map(|d| (d.saturating_duration_since(now), DeadlineKind::Paste));
        let internal = match (esc_remaining, paste_remaining) {
            (None, None) => None,
            (Some(e), None) => Some(e),
            (None, Some(p)) => Some(p),
            (Some(e), Some(p)) => Some(if e.0 <= p.0 { e } else { p }),
        };
        match (timeout, internal) {
            (None, None) => (None, DeadlineKind::None),
            (Some(t), None) => (Some(t), DeadlineKind::None),
            (None, Some((d, kind))) => (Some(d), kind),
            (Some(t), Some((d, kind))) => {
                if d <= t {
                    (Some(d), kind)
                } else {
                    (Some(t), DeadlineKind::None)
                }
            }
        }
    }
}

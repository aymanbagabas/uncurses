//! Wakeable event source shared by synchronous and asynchronous readers.
//!
//! ## Purpose
//!
//! [`EventSource`] owns the input handle, waits for platform readiness, feeds a
//! [`Decoder`], and queues typed [`Event`] values. It is the entry
//! point for applications that want blocking, timeout-based, or wakeable event
//! reads.
//!
//! ```text
//! input fd / HANDLE ─┬─▶ Poller ── ready ──▶ read bytes ──▶ Decoder
//! wake handle ───────┤                         │              │
//! SIGWINCH pipe ─────┘                         └──────────────┴─▶ queue
//!      (Unix only)        deadlines: ESC timeout and paste idle timeout
//! ```
//!
//! ## Key types
//!
//! * [`Input`] describes the platform capabilities required from an input
//!   handle.
//! * [`EventSource`] stores the decoder, bounded pending-byte buffer, readiness
//!   poller, event queue, timeout deadlines, and resize state.
//! * [`Waker`] is a cloneable handle that interrupts an in-progress wait from
//!   another thread.
//!
//! ## Lifecycle
//!
//! Construct with [`EventSource::new`] on the platform-specific impl. Call
//! [`EventSource::poll`] to wait up to a timeout, drain queued events with
//! [`EventSource::try_read`], or call [`EventSource::read`] to block until one
//! event arrives. [`EventSource::unread`] can put an unrelated event back while
//! code waits for a specific terminal reply.
//!
//! ## Gotchas
//!
//! [`EventSource::try_read`] is purely non-blocking: it only pops the queue.
//! [`EventSource::poll`] performs I/O and timeout handling. The effective poll
//! wait is shortened when a partial `ESC` sequence or open paste has an internal
//! deadline, so a long caller timeout does not delay disambiguation.
use std::collections::VecDeque;
use std::io;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::AsFd;
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
use crate::terminal::Winsize;

/// Platform-specific capabilities required from a Unix event input handle.
///
/// The handle must implement [`Read`], expose an fd through [`AsFd`], and be
/// safe to move to the helper thread used by async streams. Any type satisfying
/// those bounds implements this trait automatically.
///
/// Users normally do not implement this trait manually; pass the input half
/// returned by the terminal API to [`EventSource::new`].
#[cfg(unix)]
pub trait Input: Read + AsFd + Send {}
#[cfg(unix)]
impl<T: Read + AsFd + Send> Input for T {}

/// Platform-specific capabilities required from a Windows event input handle.
///
/// The handle must implement [`Read`], expose a console `HANDLE` through
/// [`AsHandle`], and be safe to move to the helper thread used by async streams.
/// Any type satisfying those bounds implements this trait automatically.
///
/// Users normally do not implement this trait manually; pass the input half
/// returned by the terminal API to [`EventSource::new`].
#[cfg(windows)]
pub trait Input: Read + AsHandle + Send {}
#[cfg(windows)]
impl<T: Read + AsHandle + Send> Input for T {}

/// Default read buffer capacity. This is the hard cap on any single
/// sequence; the backing buffer is allocated once at construction.
pub(super) const DEFAULT_BUFFER_CAPACITY: usize = 4096;

/// Readiness slots the platform poller reports, in fixed index order.
/// Unix watches `[input, wake, winch]`; Windows watches `[input, wake]`
/// (resize arrives in-band as a console record, so there is no winch fd).
#[cfg(unix)]
pub(super) const READY_SLOTS: usize = 3;
#[cfg(windows)]
pub(super) const READY_SLOTS: usize = 2;
/// Index of the input handle in the readiness slice.
#[cfg(any(unix, windows))]
pub(super) const READY_INPUT: usize = 0;
/// Index of the wake handle in the readiness slice.
#[cfg(any(unix, windows))]
pub(super) const READY_WAKE: usize = 1;
/// Index of the SIGWINCH pipe in the readiness slice (Unix only).
#[cfg(unix)]
pub(super) const READY_WINCH: usize = 2;

/// Default escape-sequence timeout.
///
/// When the pending buffer holds a partial `ESC`-prefixed sequence or 8-bit C1
/// introducer and no continuation arrives within this window, the source asks
/// the decoder to resolve the buffered bytes as best-effort events. This is how
/// a physical Escape key is distinguished from an Alt-prefixed key or CSI-style
/// control sequence.
///
/// Applications that prefer more responsive Escape handling can lower this
/// value with [`EventSource::with_esc_timeout`]; applications that need to
/// tolerate slow byte delivery can raise it.
pub const DEFAULT_ESC_TIMEOUT: Duration = Duration::from_millis(50);

/// Default bracketed-paste idle timeout.
///
/// When a paste has been opened (a `PasteStart` was emitted) and no
/// further input arrives within this window, the source synthesises a
/// `PasteEnd`, flushes any held-back bytes as a final `PasteChunk`,
/// and clears the decoder's paste state. Guards against terminators
/// that never arrive (truncated stream, malformed input).
pub const DEFAULT_PASTE_IDLE_TIMEOUT: Duration = Duration::from_secs(2);

/// Cloneable handle that interrupts an in-progress [`EventSource::poll`] or
/// [`EventSource::read`].
///
/// A waker is bound to one source at construction time. Calling [`Waker::wake`]
/// makes the source's readiness wait return `Interrupted`; multiple wake calls
/// may coalesce into one interruption. The handle is cheap to clone and can be
/// sent to other threads.
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

    /// Read end of the wake self-pipe. Lives behind the same `Arc` as the
    /// write end so it is never closed while a writer survives.
    #[cfg(unix)]
    pub(super) fn pipe_read_fd(&self) -> std::os::fd::RawFd {
        self.inner.read_fd()
    }

    /// Interrupt the [`EventSource`] this waker is bound to.
    ///
    /// A blocked [`EventSource::poll`] returns `Ok(false)` and a blocked
    /// [`EventSource::read`] returns an [`io::ErrorKind::Interrupted`] error.
    /// The wake does not enqueue an [`Event`] and does not mutate decoder state.
    ///
    /// Returns any platform error produced while signalling the wake handle.
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
/// `EventSource` is the synchronous owner of terminal input. It stores pending
/// bytes, a `Decoder`, an event queue, deadline state for ambiguous `ESC`
/// prefixes and open bracketed pastes, and the platform handles needed for
/// wakeups and resize notifications.
///
/// Construct it with [`EventSource::new`]. Use [`EventSource::poll`] to perform
/// I/O and wait for queued events, [`EventSource::try_read`] to pop an already
/// queued event, or [`EventSource::read`] to block until one event is available.
/// The type is generic over the platform [`Input`] handle.
pub struct EventSource<I>
where
    I: Input,
{
    /// Owned input handle. Used both as the byte source (`Read`) and, on
    /// Unix, as the readiness target (its fd is registered with the
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
    /// Shared readiness poller watching `[input, wake_rx, winch_rx]` (in
    /// that index order). Held behind `Arc` so an [`super::EventStream`]
    /// reader thread can wait on it lock-free while the source's decode
    /// state is mutated under a separate lock.
    #[cfg(unix)]
    pub(super) poller: Arc<dyn Poller>,
    /// Active SIGWINCH subscription, and owner of the pipe the handler wakes.
    /// The pipe is leased from a process-lifetime pool rather than owned here,
    /// so no descriptor the handler may be about to write to can be closed —
    /// see the `PIPES` pool in the sigwinch module.
    #[cfg(unix)]
    pub(super) winch_sub: winch::Subscription,
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

// Drop ordering note for Unix: the winch pipe is leased from a
// process-lifetime pool and is never closed, so dropping the subscription
// only frees the slot. Field drop order is therefore not load-bearing here.

// ---------------------------------------------------------------------------
// Shared methods (platform-agnostic)
// ---------------------------------------------------------------------------

impl<I> EventSource<I>
where
    I: Input,
{
    /// Return the configured escape-sequence timeout.
    ///
    /// This is the duration used to disambiguate a physical Escape key from an
    /// Alt-prefixed key or a partial control sequence. Reading it has no side
    /// effects and never performs I/O.
    pub fn esc_timeout(&self) -> Duration {
        self.esc_timeout
    }

    /// Set the escape-sequence timeout.
    ///
    /// `timeout` is how long the source waits for a continuation byte before a
    /// buffered partial escape sequence is force-resolved. The default is
    /// [`DEFAULT_ESC_TIMEOUT`]. A zero duration makes ambiguous prefixes expire
    /// on the next poll cycle.
    ///
    /// This is a consuming builder intended to be chained after
    /// [`EventSource::new`]. It does not inspect or clear any already-buffered
    /// input and never panics.
    pub fn with_esc_timeout(mut self, timeout: Duration) -> Self {
        self.esc_timeout = timeout;
        self
    }

    /// Set the idle timeout for an open bracketed paste.
    ///
    /// If no further input arrives before `timeout`, the source flushes any
    /// held bytes as a final [`Event::PasteChunk`], synthesizes
    /// [`Event::PasteEnd`], and leaves paste mode. Passing `None` disables this
    /// safety net and waits indefinitely for a real terminator. The default is
    /// `Some(`[`DEFAULT_PASTE_IDLE_TIMEOUT`]`)`.
    ///
    /// This is a consuming builder intended to be chained after
    /// [`EventSource::new`]. It never panics.
    pub fn with_paste_idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.paste_idle_timeout = timeout;
        self
    }

    /// Return a cloneable [`Waker`] bound to this source.
    ///
    /// Use the returned handle from another thread to interrupt a blocking
    /// [`EventSource::poll`] or [`EventSource::read`]. Cloning the waker does
    /// not clone the source or its input handle.
    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }

    /// Return a clone of the shared [`Poller`] handle.
    ///
    /// The poller is `Arc`-wrapped so a waiter thread can block on readiness
    /// without holding the source mutex. Both the underlying epoll and kqueue
    /// registrations are level-triggered, so a concurrent poll from a waiter
    /// and a subsequent drain from the owner both observe the same readiness.
    #[cfg(feature = "async")]
    pub(super) fn poller(&self) -> Arc<dyn Poller> {
        Arc::clone(&self.poller)
    }

    /// Return whether out-of-band resize handling is enabled.
    ///
    /// On Unix, `true` means a readable SIGWINCH pipe can produce
    /// [`Event::Resize`]. On Windows, resize records are delivered in-band and
    /// this flag has no practical effect.
    pub fn handle_resize(&self) -> bool {
        self.handle_resize
    }

    /// Control whether the source delivers [`Event::Resize`] from the
    /// kernel's out-of-band window-resize notification (`SIGWINCH` on
    /// Unix). Defaults to `true`.
    ///
    /// Set this to `false` after enabling in-band resize reports (DEC
    /// mode 2048): the terminal then reports size changes in-band as
    /// `CSI 48 t`, which the decoder surfaces as [`Event::Resize`], so
    /// leaving the `SIGWINCH` path on would deliver each resize twice.
    /// Restore it to `true` when in-band reporting is disabled again.
    ///
    /// No effect on Windows, where resize is always delivered in-band
    /// through the decoder.
    pub fn set_handle_resize(&mut self, enable: bool) {
        self.handle_resize = enable;
    }

    /// Wait up to `timeout` for at least one event to become available.
    ///
    /// This method performs I/O, drains decoder output into the internal queue,
    /// handles resize notifications, and resolves any expired ESC or paste
    /// deadlines. `None` means block until an event or wake; `Some(Duration::ZERO)`
    /// means perform a non-blocking readiness pass.
    ///
    /// Returns:
    ///
    /// * `Ok(true)` when the queue has at least one event;
    /// * `Ok(false)` when the timeout elapsed or a paired [`Waker`] interrupted
    ///   the wait without producing an event;
    /// * `Err(_)` for fatal input or platform readiness errors.
    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        if !self.queue.is_empty() {
            return Ok(true);
        }
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let remaining = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            match self.fill(remaining) {
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

    /// Return the next queued event without performing I/O.
    ///
    /// This only pops the internal queue. Call [`EventSource::poll`] first when
    /// the queue may be empty but input could be ready. Returns `None` when no
    /// event is currently queued.
    pub fn try_read(&mut self) -> Option<Event> {
        self.queue.pop_front()
    }

    /// One read-decode cycle: resolve any overdue decode deadline, wait up
    /// to `timeout` for readiness, then service it. Winch surfaces a
    /// [`Event::Resize`]; a [`Waker`] surfaces `Err(Interrupted)` without
    /// touching decode state; ready input is drained and decoded; a wait
    /// that returns with no input ready resolves the decode deadline it was
    /// tightened to. Fills [`Self::queue`].
    pub(super) fn fill(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        // Resolve an already-overdue deadline before reading, so a late
        // continuation byte cannot merge with a sequence that has expired.
        self.expire_elapsed();
        if !self.queue.is_empty() {
            return Ok(());
        }

        let effective = self.effective_timeout(timeout);
        let mut ready = [false; READY_SLOTS];
        self.poller.poll(&mut ready, effective)?;

        #[cfg(unix)]
        if ready[READY_WINCH] {
            self.handle_winch();
        }

        if ready[READY_WAKE] {
            self.drain_wake();
            return Err(io::Error::new(io::ErrorKind::Interrupted, "wake"));
        }

        if ready[READY_INPUT] {
            self.drain_input()?;
        } else {
            // The wait was tightened to a decode deadline that has elapsed.
            self.expire_elapsed();
        }
        Ok(())
    }

    /// Block until the next event is available, then return it.
    ///
    /// This repeatedly checks the queue and calls [`EventSource::poll`] with no
    /// caller timeout. It returns the next [`Event`] on success.
    ///
    /// Returns [`io::ErrorKind::Interrupted`] if a paired [`Waker`] fired while
    /// waiting, and propagates fatal input/readiness errors.
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

    /// Return an event to the front of the queue, so the next
    /// [`read`](Self::read) / [`try_read`](Self::try_read) yields it before
    /// anything already queued. Use to put back an event read while waiting
    /// for a specific reply (e.g. a cursor-position report), preserving it
    /// for normal delivery. Restore a batch in original order by unreading
    /// in reverse.
    pub fn unread(&mut self, event: Event) {
        self.queue.push_front(event);
    }

    /// Push a freshly produced event onto the queue.
    pub(super) fn emit(&mut self, ev: Event) {
        self.queue.push_back(ev);
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
                self.emit(ev);
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
            self.emit(ev);
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
            self.emit(Event::PasteChunk(bytes));
        }
        if let Some(ev) = self.parser.end_paste() {
            self.emit(ev);
        }
    }

    /// Force-exit bracketed paste mode.
    ///
    /// If the decoder is currently inside a paste, this flushes any pending
    /// bytes as a [`Event::PasteChunk`], queues [`Event::PasteEnd`], and clears
    /// paste state so subsequent bytes parse as ordinary input. Use it when an
    /// embedding application enforces a paste size cap, user cancellation, or a
    /// custom watchdog. It has no effect outside paste mode and never panics.
    pub fn end_paste(&mut self) {
        self.expire_paste();
    }

    /// Resolve any decode deadline that has already elapsed, before more
    /// bytes are read. A buffered partial `ESC` past its window becomes a
    /// bare `Esc`; an idle bracketed paste past its window is force-closed.
    /// Run before draining input so a late continuation byte can't merge
    /// with a sequence whose deadline already passed, and again after a
    /// wait that returned no input. A no-op when no deadline is overdue;
    /// the two cases are mutually exclusive (either mid-paste or holding a
    /// partial escape, never both).
    pub(super) fn expire_elapsed(&mut self) {
        let now = Instant::now();
        if self.parser.in_paste() {
            if self.paste_deadline.is_some_and(|d| d <= now) {
                self.expire_paste();
            }
        } else if self.esc_deadline.is_some_and(|d| d <= now) {
            self.expire_partial();
        }
    }

    /// Effective wait for the next readiness poll: the caller's `timeout`
    /// tightened to the nearest decode deadline (ESC or paste-idle,
    /// whichever is sooner) so a buffered partial sequence resolves
    /// promptly even when the caller asked to block longer.
    pub(super) fn effective_timeout(&self, timeout: Option<Duration>) -> Option<Duration> {
        let now = Instant::now();
        let deadline = match (self.esc_deadline, self.paste_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let internal = deadline.map(|d| d.saturating_duration_since(now));
        match (timeout, internal) {
            (Some(t), Some(i)) => Some(t.min(i)),
            (None, i) => i,
            (t, None) => t,
        }
    }
}

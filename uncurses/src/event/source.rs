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

    /// Return the next queued event, or `None` if the queue is empty.
    /// Does not perform I/O.
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

    /// Return an event to the front of the queue, so the next
    /// [`read`](Self::read) or [`try_read`](Self::try_read) yields it
    /// first. The inverse of [`try_read`](Self::try_read): use it to put
    /// back an event taken while looking for another one.
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

    /// Public escape hatch: force-exit bracketed paste mode. Useful
    /// when the embedding application detects misuse (size cap, user
    /// cancel, watchdog) and wants to recover the input stream.
    ///
    /// Has no effect when the decoder is not in paste.
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

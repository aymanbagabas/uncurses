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
//! Construct with [`EventSource::new`] or [`EventSource::new_with_flags`]. Call
//! [`EventSource::poll`] to wait up to a timeout, drain queued events with
//! [`EventSource::try_read`], or call [`EventSource::read`] to block until one
//! event arrives. [`EventSource::unread`] can put an unrelated event back while
//! code waits for a specific terminal reply.
//!
//! ## Gotchas
//!
//! [`EventSource::try_read`] is purely non-blocking: it only pops the queue.
//! [`EventSource::poll`] performs I/O and timeout handling. The effective poll
//! wait is shortened when a partial sequence, string, or paste has an internal
//! deadline, so a long caller timeout does not delay disambiguation.
use std::collections::VecDeque;
use std::io;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bitflags::bitflags;

#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(windows)]
use std::os::windows::io::AsHandle;

use super::decode::{Decoder, DecoderFlags, is_c1_introducer};
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

bitflags! {
    /// Input source configuration, separate from [`DecoderFlags`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct SourceFlags: u8 {
        /// On Windows, serialize console key records as VT input.
        ///
        /// Set this flag when the console uses `ENABLE_VIRTUAL_TERMINAL_INPUT`.
        const VT_INPUT = 1 << 0;
    }
}

/// Default read buffer capacity. This is the hard cap on any single
/// sequence; the backing buffer is allocated once at construction.
pub(super) const DEFAULT_BUFFER_CAPACITY: usize = 8192;

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
/// Fresh input resets the deadline while a sequence remains incomplete.
/// Once bytes follow an OSC, DCS, APC, SOS, or PM introducer, the string waits
/// for its terminator under [`DEFAULT_STRING_IDLE_TIMEOUT`].
/// An introducer alone still uses the Escape timeout.
/// A timed-out two-byte introducer such as `ESC ]` resolves to `Alt+]`.
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

/// Default idle timeout for an OSC, DCS, APC, SOS, or PM body.
///
/// Fresh input resets this deadline. On expiry, the source emits the incomplete
/// sequence, including its introducer, as [`Event::Unknown`] and resumes normal
/// decoding. Bytes that arrive after recovery count as new input.
pub const DEFAULT_STRING_IDLE_TIMEOUT: Duration = Duration::from_secs(2);

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
    pub(super) fn new() -> io::Result<Self> {
        #[cfg(unix)]
        return Ok(Self::from_unix_inner(Arc::new(UnixWakerInner::new()?)));
        #[cfg(windows)]
        return Ok(Self::from_windows_inner(
            Arc::new(WindowsWakerInner::new()?),
        ));
    }

    pub(super) fn poll_fd(&self) -> super::poll::PollFd {
        #[cfg(unix)]
        return self.inner.read_fd();
        #[cfg(windows)]
        return self.inner.raw_handle();
    }

    #[cfg(feature = "async")]
    pub(super) fn clear(&self) -> io::Result<()> {
        self.inner.clear()
    }

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
/// prefixes, strings, and bracketed pastes, and the platform handles needed for
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

    pub(super) flags: SourceFlags,
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
    pub(super) string_idle_timeout: Option<Duration>,
    pub(super) string_deadline: Option<Instant>,
    #[cfg(feature = "async")]
    pub(super) deadline_wakers: Vec<std::sync::Weak<super::stream::DeadlineWake>>,
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
    /// Build an event source for `input` with empty source flags.
    ///
    /// See [`Self::new_with_flags`] for platform requirements and construction errors.
    pub fn new(input: I) -> io::Result<Self> {
        Self::new_with_flags(input, SourceFlags::empty())
    }

    /// Return the input source flags.
    pub fn flags(&self) -> SourceFlags {
        self.flags
    }

    /// Set the input source flags without changing terminal modes or decoder flags.
    ///
    /// On Windows, a [`SourceFlags::VT_INPUT`] change discards incomplete UTF-16 pairs.
    pub fn set_flags(&mut self, flags: SourceFlags) {
        #[cfg(windows)]
        if (self.flags ^ flags).contains(SourceFlags::VT_INPUT) {
            self.pending_high_surrogate = [None, None];
        }
        self.flags = flags;
    }

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
    /// [`DEFAULT_ESC_TIMEOUT`]. Fresh input resets the deadline for incomplete
    /// sequences. Ready continuation bytes take priority over their expiry.
    /// A lone Escape expires before the next read once its deadline passes.
    ///
    /// A zero duration resolves a lone Escape on the next poll cycle and other
    /// ambiguous prefixes when a poll finds no continuation input.
    /// String bodies wait for their terminators regardless of this timeout.
    ///
    /// This is a consuming builder intended to be chained after
    /// [`EventSource::new`]. It does not inspect or clear any already-buffered
    /// input and never panics.
    pub fn with_esc_timeout(mut self, timeout: Duration) -> Self {
        self.esc_timeout = timeout;
        self
    }

    /// Choose how the ambiguous legacy keys are read.
    ///
    /// LF reads as Ctrl+J by default. Set [`DecoderFlags::LF_IS_ENTER`] to
    /// read it as Enter instead. See [`DecoderFlags`] for all mappings.
    ///
    /// This is a consuming builder intended to be chained after
    /// [`EventSource::new`]. It never panics.
    pub fn with_decoder_flags(mut self, flags: DecoderFlags) -> Self {
        self.set_decoder_flags(flags);
        self
    }

    /// Choose how the ambiguous legacy keys are read, on a source already in
    /// use.
    ///
    /// The same choice [`with_decoder_flags`](Self::with_decoder_flags)
    /// makes, for a caller holding the source rather than building it.
    /// Buffered input is left alone: the flags decide how a byte is named,
    /// not how it is framed.
    pub fn set_decoder_flags(&mut self, flags: DecoderFlags) {
        self.parser.set_flags(flags);
    }

    /// How the ambiguous legacy keys are being read.
    pub fn decoder_flags(&self) -> DecoderFlags {
        self.parser.flags()
    }

    /// Return the decoder's current Backarrow mode (DECBKM).
    ///
    /// Defaults to reset (`false`).
    pub fn backspace_mode(&self) -> bool {
        self.parser.backspace_mode()
    }

    /// Select Backarrow mode for future legacy input without terminal I/O.
    ///
    /// Set (`true`) selects BS (`0x08`) for Backspace; reset (`false`) selects
    /// DEL (`0x7f`). [`DecoderFlags`] choose the ambiguous interpretations.
    /// Mode reports are returned to the caller for handling. Already queued
    /// events keep their decoded values.
    ///
    /// Use [`Program::set_backspace_mode`](crate::program::Program::set_backspace_mode)
    /// to change both the terminal and decoder.
    pub fn set_backspace_mode(&mut self, enabled: bool) {
        self.parser.set_backspace_mode(enabled);
    }

    /// Set the idle timeout for an open bracketed paste.
    ///
    /// If no further input arrives before `timeout`, the source flushes any
    /// held bytes as a final [`Event::PasteChunk`], synthesizes
    /// [`Event::PasteEnd`], and leaves paste mode. Passing `None` disables this
    /// safety net and waits indefinitely for a real terminator. The default is
    /// `Some(`[`DEFAULT_PASTE_IDLE_TIMEOUT`]`)`.
    ///
    /// Changing this setting during a paste restarts its idle wait.
    pub fn with_paste_idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.set_paste_idle_timeout(timeout);
        self
    }

    /// Set the paste idle timeout on an existing source.
    ///
    /// This applies the same policy as
    /// [`with_paste_idle_timeout`](Self::with_paste_idle_timeout).
    /// Changing it during a paste restarts the idle wait; `None` disables it.
    /// Live async streams refresh their readiness waits when the deadline changes.
    pub fn set_paste_idle_timeout(&mut self, timeout: Option<Duration>) {
        #[cfg(feature = "async")]
        let previous = self.paste_deadline;
        self.paste_idle_timeout = timeout;
        self.paste_deadline = if self.parser.in_paste() {
            timeout.map(|t| Instant::now() + t)
        } else {
            None
        };
        #[cfg(feature = "async")]
        if self.paste_deadline != previous {
            self.wake_deadline_waiters();
        }
    }

    /// Set the idle timeout for an OSC, DCS, APC, SOS, or PM body.
    ///
    /// Fresh input resets the deadline. On expiry, the source emits the whole
    /// incomplete sequence as [`Event::Unknown`] and resumes normal decoding.
    /// Later bytes count as new input, even if they belong to the expired string.
    ///
    /// The default is `Some(`[`DEFAULT_STRING_IDLE_TIMEOUT`]`)`.
    /// Passing `None` disables the idle timeout.
    /// Changing this setting during a string restarts its idle wait.
    pub fn with_string_idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.set_string_idle_timeout(timeout);
        self
    }

    /// Set the string idle timeout on an existing source.
    ///
    /// This applies the same policy as
    /// [`with_string_idle_timeout`](Self::with_string_idle_timeout).
    /// Changing it during a string restarts the idle wait; `None` disables it.
    /// Live async streams refresh their readiness waits when the deadline changes.
    pub fn set_string_idle_timeout(&mut self, timeout: Option<Duration>) {
        #[cfg(feature = "async")]
        let previous = self.string_deadline;
        self.string_idle_timeout = timeout;
        self.string_deadline = if self.parser.waiting_for_terminator() {
            timeout.map(|t| Instant::now() + t)
        } else {
            None
        };
        #[cfg(feature = "async")]
        if self.string_deadline != previous {
            self.wake_deadline_waiters();
        }
    }

    #[cfg(feature = "async")]
    fn wake_deadline_waiters(&mut self) {
        self.deadline_wakers.retain(|weak| {
            if let Some(waker) = weak.upgrade() {
                waker.notify();
                true
            } else {
                false
            }
        });
    }

    /// Return a cloneable [`Waker`] bound to this source.
    ///
    /// Use the returned handle from another thread to interrupt a blocking
    /// [`EventSource::poll`] or [`EventSource::read`]. Cloning the waker does
    /// not clone the source or its input handle.
    pub fn waker(&self) -> Waker {
        self.waker.clone()
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
    /// A platform whose wait takes a whole number of milliseconds rounds a
    /// fractional millisecond up, so such a wait can return up to a
    /// millisecond after the deadline.
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

    /// One read-decode cycle: resolve an overdue lone Escape or paste deadline,
    /// wait up to `timeout` for readiness, then service it. Winch surfaces a
    /// [`Event::Resize`]; a [`Waker`] surfaces `Err(Interrupted)` without
    /// touching decode state; ready input is drained and decoded; a wait
    /// that returns with no input ready resolves the decode deadline it was
    /// tightened to. Fills [`Self::queue`].
    pub(super) fn fill(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        // A late key must not join an expired lone Escape. Partial sequences
        // get a readiness check first, since continuation bytes may be queued.
        if self.pending.slice() == b"\x1b" || self.parser.in_paste() {
            self.expire_elapsed();
        }
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

    /// Feed serialized console bytes in bounded batches.
    #[cfg(any(windows, test))]
    pub(super) fn feed_bytes(&mut self, mut bytes: &[u8]) {
        #[cfg(debug_assertions)]
        crate::trace::tee_input(bytes);
        while !bytes.is_empty() {
            if self.pending.is_full() {
                self.pending.clear();
                self.esc_deadline = None;
            }
            let count = self.pending.append(bytes);
            bytes = &bytes[count..];
            self.drain_parser();
        }
    }

    /// Drive the parser as far as it will go against the bytes
    /// currently in `pending`, pushing extracted events onto the
    /// queue and arming the appropriate timeout deadline.
    ///
    /// Fresh input refreshes the deadline selected by the remaining decoder
    /// state: paste idle, string idle, or Escape disambiguation.
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
            self.string_deadline = None;
            self.paste_deadline = self.paste_idle_timeout.map(|t| Instant::now() + t);
            return;
        }

        // Not in paste: clear paste deadline; arm esc deadline if a
        // partial sequence sits at the head.
        self.paste_deadline = None;
        if self.parser.waiting_for_terminator() {
            self.esc_deadline = None;
            self.string_deadline = self.string_idle_timeout.map(|t| Instant::now() + t);
            return;
        }
        self.string_deadline = None;
        let Some(b0) = self.pending.first() else {
            self.esc_deadline = None;
            return;
        };
        let armable = b0 == 0x1b || is_c1_introducer(b0);
        if armable {
            self.esc_deadline = Some(Instant::now() + self.esc_timeout);
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
        while !self.parser.waiting_for_terminator() {
            let Some(b0) = self.pending.first() else {
                break;
            };
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

    /// Resolve any decode deadline that has already elapsed.
    /// A buffered partial `ESC` past its window becomes a
    /// bare `Esc`; idle pastes and strings past their windows are force-closed.
    /// Run before input for a lone Escape or paste, and after a wait that
    /// returned no input for other partial sequences.
    /// A no-op when no deadline is overdue;
    /// paste, string, and Escape deadlines govern separate decoder states.
    pub(super) fn expire_elapsed(&mut self) {
        let now = Instant::now();
        if self.parser.in_paste() {
            if self.paste_deadline.is_some_and(|d| d <= now) {
                self.expire_paste();
            }
        } else if self.parser.waiting_for_terminator() {
            if self.string_deadline.is_some_and(|d| d <= now) {
                let bytes = self.pending.slice().to_vec();
                self.pending.clear();
                self.drain_parser();
                self.emit(Event::Unknown(bytes));
            }
        } else if self.esc_deadline.is_some_and(|d| d <= now) {
            self.expire_partial();
        }
    }

    /// Effective wait for the next readiness poll: the caller's `timeout`
    /// tightened to the nearest decode deadline (Escape, paste, or string)
    /// so a buffered partial sequence resolves promptly even when the caller
    /// asked to block longer.
    pub(super) fn effective_timeout(&self, timeout: Option<Duration>) -> Option<Duration> {
        let now = Instant::now();
        let deadline = [self.esc_deadline, self.paste_deadline, self.string_deadline]
            .into_iter()
            .flatten()
            .min();
        let internal = deadline.map(|d| d.saturating_duration_since(now));
        match (timeout, internal) {
            (Some(t), Some(i)) => Some(t.min(i)),
            (None, i) => i,
            (t, None) => t,
        }
    }
}

#[cfg(all(test, unix))]
mod drag_drop_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn drag_drop_maximum_payload_fits_the_source_buffer() {
        let (reader, mut writer) = io::pipe().unwrap();
        let mut source = EventSource::new(reader).unwrap();
        let payload = vec![b'A'; 4096];
        let body = [b"72;t=r:x=1;".as_slice(), &payload].concat();
        writer.write_all(b"\x1b]").unwrap();
        writer.write_all(&body).unwrap();
        writer.write_all(b"\x1b\\").unwrap();
        assert_eq!(
            source.read().unwrap(),
            Event::DragDrop {
                options: vec![("t".into(), "r".into()), ("x".into(), "1".into())],
                payload,
            }
        );
    }

    #[test]
    fn drag_drop_batches_and_split_terminators_preserve_events() {
        let (reader, _writer) = io::pipe().unwrap();
        let mut source = EventSource::new(reader).unwrap();
        let payload = vec![b'A'; 4096];
        let frame = [b"\x1b]72;t=r:x=1;".as_slice(), &payload, b"\x1b\\"].concat();
        source.feed_bytes(&frame[..frame.len() - 1]);
        assert!(source.queue.is_empty());
        source.feed_bytes(&[b"\\".as_slice(), &frame, &frame, b"\x1b]72;t=q\x1b\\z"].concat());
        for _ in 0..3 {
            assert_eq!(
                source.queue.pop_front(),
                Some(Event::DragDrop {
                    options: vec![("t".into(), "r".into()), ("x".into(), "1".into())],
                    payload: payload.clone(),
                })
            );
        }
        assert!(matches!(
            source.queue.pop_front(),
            Some(Event::DragDrop { .. })
        ));
        assert!(matches!(source.queue.pop_front(), Some(Event::KeyPress(_))));
        assert!(source.queue.is_empty());
    }
}

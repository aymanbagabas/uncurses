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
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
#[cfg(windows)]
use std::os::windows::io::AsHandle;

use super::decode::{
    Apc, Csi, Dcs, Decoder, DecoderFlags, HandlerId, Osc, Pm, Sos, Ss3, is_c1_introducer,
};
use super::pending::Pending;
#[cfg(unix)]
use super::poll::{Poll, PollFd, Poller};
#[cfg(unix)]
use super::sigwinch as winch;
#[cfg(windows)]
use super::source_windows::WindowsWakerInner;
use crate::event::Event;
#[cfg(unix)]
use crate::terminal::size::{Winsize, get_window_size};

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

/// Default read buffer capacity used by event sources.
pub const DEFAULT_BUFFER_CAPACITY: usize = 4096;

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

/// Construction-time options for an [`EventSource`].
///
/// All fields are optional knobs with documented defaults.
#[derive(Debug, Clone)]
pub struct Options {
    /// Escape-sequence timeout — how long to wait for a continuation
    /// byte before treating a buffered partial sequence as a bare
    /// `Esc` keypress. Defaults to 50 ms.
    pub esc_timeout: Duration,
    /// Idle timeout for a bracketed paste with no closing terminator.
    /// `None` disables; the source will then wait indefinitely for a
    /// real `PasteEnd`. Defaults to `Some(2 s)`.
    pub paste_idle_timeout: Option<Duration>,
    /// Read buffer capacity in bytes. Sequences exceeding this size
    /// are dropped silently. Defaults to 4 KiB.
    pub buffer_capacity: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            esc_timeout: DEFAULT_ESC_TIMEOUT,
            paste_idle_timeout: Some(DEFAULT_PASTE_IDLE_TIMEOUT),
            buffer_capacity: DEFAULT_BUFFER_CAPACITY,
        }
    }
}

impl Options {
    pub fn new() -> Self {
        Self::default()
    }
}

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

#[cfg(unix)]
pub(super) struct UnixWakerInner {
    /// Write end of the self-pipe. Non-blocking; closed on drop.
    tx: OwnedFd,
}

#[cfg(unix)]
impl UnixWakerInner {
    fn wake(&self) -> io::Result<()> {
        let buf = [b'w'];
        loop {
            let n = unsafe { libc::write(self.tx.as_raw_fd(), buf.as_ptr() as *const _, 1) };
            if n < 0 {
                let err = io::Error::last_os_error();
                match err.kind() {
                    io::ErrorKind::Interrupted => continue,
                    // Pipe full — an earlier wake byte is already pending,
                    // which is all the consumer needs.
                    io::ErrorKind::WouldBlock => return Ok(()),
                    _ => return Err(err),
                }
            }
            return Ok(());
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

    // --- Unix-only state ---
    #[cfg(unix)]
    pub(super) poller: Poller,
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

    /// Replace the escape-sequence timeout. Takes effect on the next
    /// armed deadline; an already-armed deadline is left as is.
    pub fn set_esc_timeout(&mut self, timeout: Duration) {
        self.esc_timeout = timeout;
    }

    /// Cloneable [`Waker`] bound to this source.
    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }

    /// Read-only access to the underlying [`Decoder`] (for state queries
    /// like [`Decoder::in_paste`]). Handler registration goes through the
    /// `on_*` methods on `EventSource` directly.
    pub fn decoder(&self) -> &Decoder {
        &self.parser
    }

    /// Register a hook for unrecognised CSI sequences. See
    /// [`Decoder::on_csi`].
    pub fn on_csi<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a, 'b> Fn(&'b Csi<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        self.parser.on_csi(f)
    }

    /// Register a hook for unrecognised SS3 sequences. See
    /// [`Decoder::on_ss3`].
    pub fn on_ss3<F>(&mut self, f: F) -> HandlerId
    where
        F: Fn(Ss3) -> Option<Event> + Send + Sync + 'static,
    {
        self.parser.on_ss3(f)
    }

    /// Register a hook for unrecognised OSC payloads. See
    /// [`Decoder::on_osc`].
    pub fn on_osc<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a> Fn(Osc<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        self.parser.on_osc(f)
    }

    /// Register a hook for unrecognised DCS payloads. See
    /// [`Decoder::on_dcs`].
    pub fn on_dcs<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a, 'b> Fn(&'b Dcs<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        self.parser.on_dcs(f)
    }

    /// Register a hook for unrecognised APC payloads. See
    /// [`Decoder::on_apc`].
    pub fn on_apc<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a> Fn(Apc<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        self.parser.on_apc(f)
    }

    /// Register a hook for PM (Privacy Message) payloads. See
    /// [`Decoder::on_pm`].
    pub fn on_pm<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a> Fn(Pm<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        self.parser.on_pm(f)
    }

    /// Register a hook for SOS (Start Of String) payloads. See
    /// [`Decoder::on_sos`].
    pub fn on_sos<F>(&mut self, f: F) -> HandlerId
    where
        F: for<'a> Fn(Sos<'a>) -> Option<Event> + Send + Sync + 'static,
    {
        self.parser.on_sos(f)
    }

    /// Register a hook for raw bytes that don't begin any recognised
    /// sequence. See [`Decoder::on_unknown`].
    pub fn on_unknown<F>(&mut self, f: F) -> HandlerId
    where
        F: Fn(&[u8]) -> Option<Event> + Send + Sync + 'static,
    {
        self.parser.on_unknown(f)
    }

    /// Deregister a previously-registered hook by id. Returns `true` when
    /// a handler with that id was found and removed.
    pub fn remove_handler(&mut self, id: HandlerId) -> bool {
        self.parser.remove_handler(id)
    }

    /// Remove every registered hook across all categories.
    pub fn clear_handlers(&mut self) {
        self.parser.clear_handlers();
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

// ---------------------------------------------------------------------------
// Unix implementation
// ---------------------------------------------------------------------------

#[cfg(unix)]
impl<I> EventSource<I>
where
    I: Input,
{
    /// Build a new event source for `input` with default [`Options`].
    /// See [`EventSource::with_options`] for the knob-configurable variant.
    pub fn new(input: I) -> io::Result<Self> {
        Self::with_options(input, Options::default())
    }

    /// Build a new event source for `input`. The handle is also used as
    /// the `TIOCGWINSZ` target whenever SIGWINCH fires, so it must refer
    /// to the terminal whose size the caller cares about (typically the
    /// controlling tty — any fd that names it works).
    pub fn with_options(input: I, opts: Options) -> io::Result<Self> {
        let (pipe_rx, pipe_tx) = make_self_pipe()?;
        let (winch_rx, winch_tx) = make_self_pipe()?;
        let winch_sub = winch::subscribe(winch_tx.as_raw_fd())?;
        let waker = Waker::from_unix_inner(Arc::new(UnixWakerInner { tx: pipe_tx }));

        let poller = Poller::new()?;

        let capacity = opts.buffer_capacity.max(64);

        Ok(Self {
            input,
            parser: Decoder::new(DecoderFlags::empty()),
            pending: Pending::with_capacity(capacity),
            esc_timeout: opts.esc_timeout,
            esc_deadline: None,
            paste_idle_timeout: opts.paste_idle_timeout,
            paste_deadline: None,
            queue: VecDeque::with_capacity(16),
            waker,
            poller,
            pipe_rx,
            winch_rx,
            _winch_tx: winch_tx,
            _winch_sub: winch_sub,
            last_size: None,
        })
    }

    /// Single read+decode cycle. Fills [`Self::queue`]. Returns
    /// [`io::ErrorKind::Interrupted`] if a paired waker fired.
    fn pump(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let (effective, kind) = self.effective_timeout(timeout);

        let mut entries = [
            PollFd::new(self.input.as_fd().as_raw_fd()),
            PollFd::new(self.pipe_rx.as_raw_fd()),
            PollFd::new(self.winch_rx.as_raw_fd()),
        ];
        self.poller.poll(&mut entries, effective)?;
        let input_ready = entries[0].ready;
        let wake_ready = entries[1].ready;
        let winch_ready = entries[2].ready;
        let any = input_ready || wake_ready || winch_ready;

        if winch_ready {
            self.handle_winch();
        }

        if wake_ready {
            drain_pipe(self.pipe_rx.as_raw_fd());
            return Err(io::Error::new(io::ErrorKind::Interrupted, "wake"));
        }

        if input_ready {
            self.handle_input_ready()?;
        } else if !any {
            match kind {
                DeadlineKind::Esc => self.expire_partial(),
                DeadlineKind::Paste => self.expire_paste(),
                DeadlineKind::None => {}
            }
        }

        Ok(())
    }

    fn handle_input_ready(&mut self) -> io::Result<()> {
        // If the buffer is full and the parser still couldn't extract
        // an event, the contract says the buffer size is the hard cap
        // on any single sequence — drop the buffer silently and resume.
        if self.pending.is_full() {
            self.pending.clear();
            self.esc_deadline = None;
        }
        let n = match self.input.read(self.pending.spare_mut()) {
            Ok(n) => n,
            Err(e) => {
                if matches!(
                    e.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) {
                    return Ok(());
                }
                return Err(e);
            }
        };
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "input closed"));
        }
        self.pending.advance_written(n);
        #[cfg(debug_assertions)]
        {
            let s = self.pending.slice();
            crate::trace::tee_input(&s[s.len() - n..]);
        }
        self.drain_parser();
        Ok(())
    }

    fn handle_winch(&mut self) {
        drain_pipe(self.winch_rx.as_raw_fd());
        let new_size = match get_window_size(self.input.as_fd()) {
            Ok(sz) => sz,
            Err(_) => return,
        };
        if Some(new_size) == self.last_size {
            return;
        }
        self.last_size = Some(new_size);
        self.queue.push_back(Event::Resize(new_size));
    }
}

#[cfg(unix)]
fn make_self_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe(2) just produced two fresh, owned fds.
    let rx = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let tx = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    set_nonblock_cloexec(rx.as_raw_fd())?;
    set_nonblock_cloexec(tx.as_raw_fd())?;
    Ok((rx, tx))
}

#[cfg(unix)]
fn set_nonblock_cloexec(fd: i32) -> io::Result<()> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd_flags = libc::fcntl(fd, libc::F_GETFD);
        if fd_flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::fcntl(fd, libc::F_SETFD, fd_flags | libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn drain_pipe(fd: i32) {
    let mut buf = [0u8; 32];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n <= 0 {
            break;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::event::KeyCode;
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::thread;

    fn make_pipe() -> (File, File) {
        let mut fds = [0i32; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "pipe() failed");
        // SAFETY: pipe(2) produced two fresh, owned fds.
        let rx = unsafe { File::from_raw_fd(fds[0]) };
        let tx = unsafe { File::from_raw_fd(fds[1]) };
        (rx, tx)
    }

    fn write_byte(f: &File, byte: u8) {
        let n = unsafe { libc::write(f.as_raw_fd(), &byte as *const _ as *const _, 1) };
        assert_eq!(n, 1);
    }

    fn write_bytes(f: &File, bytes: &[u8]) {
        let n = unsafe { libc::write(f.as_raw_fd(), bytes.as_ptr() as *const _, bytes.len()) };
        assert_eq!(n, bytes.len() as isize);
    }

    fn new_reader(input: File) -> EventSource<File> {
        EventSource::with_options(
            input,
            Options {
                buffer_capacity: 1024,
                esc_timeout: Duration::from_millis(50),
                ..Options::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn reads_event_from_input_fd() {
        let (rx, tx) = make_pipe();
        let mut src = new_reader(rx);
        write_byte(&tx, b'a');
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        let ev = src.read().unwrap();
        match ev {
            Event::KeyPress(k) => assert_eq!(k.code, KeyCode::Char('a')),
            other => panic!("unexpected event {:?}", other),
        }
    }

    #[test]
    fn timeout_returns_none() {
        let (rx, _tx) = make_pipe();
        let mut src = new_reader(rx);
        let start = Instant::now();
        let res = src.poll(Some(Duration::from_millis(10))).unwrap();
        let elapsed = start.elapsed();
        assert!(!res);
        assert!(elapsed >= Duration::from_millis(5));
    }

    #[test]
    fn waker_interrupts_blocking_read() {
        let (rx, _tx) = make_pipe();
        let mut src = new_reader(rx);
        let waker = src.waker();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            waker.wake().unwrap();
        });
        let err = src.read().expect_err("should be Interrupted");
        handle.join().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn paste_idle_timeout_synthesizes_paste_end() {
        let (rx, tx) = make_pipe();
        let mut src = EventSource::with_options(
            rx,
            Options {
                buffer_capacity: 1024,
                paste_idle_timeout: Some(Duration::from_millis(40)),
                ..Options::default()
            },
        )
        .unwrap();
        write_bytes(&tx, b"\x1b[200~hello");
        // Drain PasteStart + initial chunk.
        let mut got_start = false;
        let mut got_chunk = false;
        for _ in 0..4 {
            if !src.poll(Some(Duration::from_millis(20))).unwrap() {
                break;
            }
            while let Some(ev) = src.try_read() {
                match ev {
                    Event::PasteStart => got_start = true,
                    Event::PasteChunk(b) => {
                        assert_eq!(b, b"hello".to_vec());
                        got_chunk = true;
                    }
                    other => panic!("unexpected pre-timeout event {:?}", other),
                }
            }
            if got_start && got_chunk {
                break;
            }
        }
        assert!(got_start && got_chunk);

        // Now stop sending data and wait past the paste-idle deadline.
        let start = Instant::now();
        assert!(src.poll(Some(Duration::from_secs(5))).unwrap());
        let ev = src.read().unwrap();
        let elapsed = start.elapsed();
        assert_eq!(ev, Event::PasteEnd);
        assert!(
            elapsed < Duration::from_millis(500),
            "elapsed = {:?}",
            elapsed
        );
    }

    #[test]
    fn paste_completes_when_terminator_arrives_within_idle_window() {
        let (rx, tx) = make_pipe();
        let mut src = EventSource::with_options(
            rx,
            Options {
                buffer_capacity: 1024,
                paste_idle_timeout: Some(Duration::from_millis(500)),
                ..Options::default()
            },
        )
        .unwrap();
        write_bytes(&tx, b"\x1b[200~hi");
        let _ = src.poll(Some(Duration::from_millis(50))).unwrap();
        while src.try_read().is_some() {}
        // Sub-timeout pause, then deliver the terminator.
        thread::sleep(Duration::from_millis(50));
        write_bytes(&tx, b"\x1b[201~");
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        let mut saw_end = false;
        while let Some(ev) = src.try_read() {
            if matches!(ev, Event::PasteEnd) {
                saw_end = true;
            }
        }
        if !saw_end {
            // Drain another pump cycle if necessary.
            let _ = src.poll(Some(Duration::from_millis(50))).unwrap();
            while let Some(ev) = src.try_read() {
                if matches!(ev, Event::PasteEnd) {
                    saw_end = true;
                }
            }
        }
        assert!(saw_end, "expected PasteEnd within the idle window");
    }

    #[test]
    fn explicit_end_paste_recovers_stream() {
        let (rx, tx) = make_pipe();
        let mut src = EventSource::with_options(
            rx,
            Options {
                buffer_capacity: 1024,
                paste_idle_timeout: None,
                ..Options::default()
            },
        )
        .unwrap();
        write_bytes(&tx, b"\x1b[200~stuck");
        let _ = src.poll(Some(Duration::from_millis(50))).unwrap();
        while src.try_read().is_some() {}

        // No terminator will arrive; force-exit.
        src.end_paste();
        let ev = src.try_read().expect("PasteEnd should be queued");
        assert_eq!(ev, Event::PasteEnd);

        // Subsequent bytes parse as normal input again.
        write_bytes(&tx, b"a");
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        let ev = src.read().unwrap();
        assert!(matches!(
            ev,
            Event::KeyPress(ref k) if k.code == KeyCode::Char('a')
        ));
    }

    #[test]
    fn paste_idle_timeout_disabled_blocks_indefinitely() {
        let (rx, tx) = make_pipe();
        let mut src = EventSource::with_options(
            rx,
            Options {
                buffer_capacity: 1024,
                paste_idle_timeout: None,
                ..Options::default()
            },
        )
        .unwrap();
        write_bytes(&tx, b"\x1b[200~partial");
        let _ = src.poll(Some(Duration::from_millis(50))).unwrap();
        while src.try_read().is_some() {}

        // With the safety net disabled, a long-but-finite caller
        // timeout should expire without synthesising PasteEnd.
        let res = src.poll(Some(Duration::from_millis(80))).unwrap();
        assert!(!res, "should time out, not synthesise PasteEnd");
        assert!(src.try_read().is_none());
    }

    #[test]
    fn esc_deadline_does_not_fire_during_paste() {
        // Pre-fix latent bug: while in paste, a partial ESC at the
        // head of the pending buffer must not synthesise Key(Esc).
        let (rx, tx) = make_pipe();
        let mut src = EventSource::with_options(
            rx,
            Options {
                buffer_capacity: 1024,
                esc_timeout: Duration::from_millis(20),
                paste_idle_timeout: Some(Duration::from_secs(5)),
            },
        )
        .unwrap();
        write_bytes(&tx, b"\x1b[200~body");
        let _ = src.poll(Some(Duration::from_millis(50))).unwrap();
        while src.try_read().is_some() {}

        // Send only the beginning of the terminator: a partial ESC
        // sequence at the head of pending. The esc_timeout (20 ms)
        // must NOT fire — only the paste timeout (5 s) governs here.
        write_bytes(&tx, b"\x1b[20");
        let _ = src.poll(Some(Duration::from_millis(80))).unwrap();
        let mut saw_esc = false;
        while let Some(ev) = src.try_read() {
            if matches!(ev, Event::KeyPress(ref k) if k.code == KeyCode::Escape) {
                saw_esc = true;
            }
        }
        assert!(!saw_esc, "esc deadline must not fire during paste");

        // Complete the terminator: paste ends cleanly.
        write_bytes(&tx, b"1~");
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        let mut saw_end = false;
        while let Some(ev) = src.try_read() {
            if matches!(ev, Event::PasteEnd) {
                saw_end = true;
            }
        }
        assert!(saw_end);
    }

    #[test]
    fn esc_deadline_tightens_long_caller_timeout() {
        let (rx, tx) = make_pipe();
        let mut src = EventSource::with_options(
            rx,
            Options {
                buffer_capacity: 1024,
                esc_timeout: Duration::from_millis(20),
                ..Options::default()
            },
        )
        .unwrap();
        write_byte(&tx, 0x1b);
        let _ = src.poll(Some(Duration::from_secs(60))).unwrap();
        let start = Instant::now();
        assert!(src.poll(Some(Duration::from_secs(60))).unwrap());
        let ev = src.read().unwrap();
        let elapsed = start.elapsed();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Escape));
        assert!(
            elapsed < Duration::from_millis(500),
            "elapsed = {:?}",
            elapsed
        );
    }

    #[test]
    fn paste_end_after_chunk_is_delivered_without_extra_input() {
        // Regression: when a paste body and its closing terminator arrive
        // in the same read, the decoder returns the chunk first and queues
        // PasteEnd on its internal pending list. The source must drain that
        // queued event in the same drain pass — otherwise PasteEnd would
        // stall until the next byte showed up.
        let (rx, tx) = make_pipe();
        let mut src = new_reader(rx);
        write_bytes(&tx, b"\x1b[200~hello\x1b[201~");
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        assert!(matches!(src.read().unwrap(), Event::PasteStart));
        assert!(matches!(src.read().unwrap(), Event::PasteChunk(ref b) if b == b"hello"));
        assert!(matches!(src.read().unwrap(), Event::PasteEnd));
    }

    #[test]
    fn sigwinch_surfaces_resize_event() {
        // SIGWINCH requires a real tty to query TIOCGWINSZ on. Dup
        // stderr — under cargo test it is typically a tty — and use
        // that fd as the input source. If stderr isn't a tty, skip.
        let stderr_fd = 2;
        let ws: libc::winsize = unsafe { std::mem::zeroed() };
        let probe = unsafe { libc::ioctl(stderr_fd, libc::TIOCGWINSZ, &ws as *const _) };
        if probe < 0 {
            return;
        }
        let stderr_dup = unsafe { libc::dup(stderr_fd) };
        assert!(stderr_dup >= 0);
        let stderr_file = unsafe { File::from_raw_fd(stderr_dup) };
        let mut src = new_reader(stderr_file);
        // Force a mismatched cached size so the SIGWINCH path surfaces
        // the dedupe-suppressed event.
        src.last_size = None;
        unsafe { libc::raise(libc::SIGWINCH) };
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        let ev = src.read().unwrap();
        assert!(matches!(ev, Event::Resize(_)));
    }
}

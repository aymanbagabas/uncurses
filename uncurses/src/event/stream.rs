//! Thread-backed event reader and concurrent query engine.
//!
//! [`EventStream`] turns a blocking [`EventSource`] into a shared,
//! thread-backed reader: a single dedicated thread does the blocking
//! readiness waits and decode, then dispatches each decoded event to
//! whoever is waiting for it. Consumers are passive — they register a
//! query observer (in the source's registry) or a waker (for the next
//! event) and park — so the input fd has exactly one owner and there is
//! never a contest over who reads it.
//!
//! This is what makes concurrent queries safe: any number of queries may
//! be in flight at once, from one thread or many. Each registers its
//! reply matcher; the reader fills each query's slot out of the event
//! flow as events decode, in whatever order the terminal sends them, and
//! leaves every event queued, in order, for [`read`](EventStream::read) —
//! a query never hides input. The same engine backs the asynchronous
//! [`Stream`] face under the `async` feature: a query slot stores a
//! [`std::task::Waker`], so a synchronous caller parks its OS thread
//! through a thread-unpark waker while an async task uses its own task
//! waker, with no separate code path.
//!
//! The source is shared (`Arc<Mutex<EventSource>>`); the reader waits for
//! readiness lock-free through a cloned [`Poller`] and only takes the
//! source lock to drain what is ready, re-checking readiness under the
//! lock so a blocking input fd never stalls the lock.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Wake, Waker as TaskWaker};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::Event;
use super::poll::Poller;
use super::query::{Query, Registrar};
use super::source::{DeadlineKind, EventSource, Input, Waker};

/// Coordination state between the reader thread and the consumers.
///
/// Decoded events live in the source's queue and query replies in the
/// source's observer registry; this carries only the next-event consumer
/// waker and the terminal error/closed signal.
#[derive(Default)]
struct Inner {
    /// Next-event consumer waker, stored while that consumer is parked.
    task: Option<TaskWaker>,
    /// A fatal reader error, surfaced once to the consumer.
    error: Option<io::Error>,
    /// Set once the source is closed (EOF/error) or the reader is dropped.
    closed: bool,
}

struct Shared {
    inner: Mutex<Inner>,
}

fn wake_task(inner: &mut Inner) {
    if let Some(w) = inner.task.take() {
        w.wake();
    }
}

/// A thread-backed reader over a shared [`EventSource`], and the engine
/// behind concurrent terminal queries.
///
/// Build one with [`EventSource::into_stream`] (standalone) or
/// [`EventStream::from_shared`] (sharing a source kept elsewhere).
/// Dropping it stops and joins the reader thread; the source itself lives
/// as long as any `Arc` to it remains.
///
/// `EventStream` is `Send + Sync` and its methods take `&self`, so it can
/// be shared across threads (e.g. `Arc<EventStream>`): each thread may
/// run [`read`](Self::read) or [`query`](Self::query) and the single
/// reader thread serves them all. Under the `async` feature it also
/// implements [`futures_core::Stream`], yielding `io::Result<Event>`.
pub struct EventStream<I: Input> {
    source: Arc<Mutex<EventSource<I>>>,
    shared: Arc<Shared>,
    /// Wakes the reader thread out of its lock-free readiness wait (for a
    /// newly submitted query or for shutdown).
    waker: Waker,
    handle: Option<JoinHandle<()>>,
}

impl<I> EventSource<I>
where
    I: Input + 'static,
{
    /// Convert this source into a thread-backed [`EventStream`].
    ///
    /// The source is wrapped in `Arc<Mutex<_>>` and shared with a reader
    /// thread. To keep a handle to the source, build the `Arc<Mutex<_>>`
    /// yourself and use [`EventStream::from_shared`].
    pub fn into_stream(self) -> EventStream<I> {
        EventStream::from_shared(Arc::new(Mutex::new(self)))
    }
}

impl<I> EventStream<I>
where
    I: Input + 'static,
{
    /// Build a stream over a source shared via `Arc<Mutex<_>>`.
    ///
    /// The caller may keep its own clone of the `Arc` to run synchronous
    /// [`EventSource::query`] calls (locking the same source) while the
    /// stream is live.
    pub fn from_shared(source: Arc<Mutex<EventSource<I>>>) -> Self {
        let (waker, poller, slots) = {
            let s = source.lock().unwrap();
            (s.waker(), s.poller(), s.poll_slot_count())
        };
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
        });
        let reader_source = Arc::clone(&source);
        let reader_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("uncurses-event-reader".to_string())
            .spawn(move || reader_loop(reader_source, poller, slots, reader_shared))
            .expect("spawn event reader thread");
        Self {
            source,
            shared,
            waker,
            handle: Some(handle),
        }
    }
}

impl<I: Input> EventStream<I> {
    /// Take the next decoded event without blocking, or `None` if none is
    /// queued yet.
    pub fn try_read(&self) -> Option<Event> {
        self.source.lock().unwrap().try_read()
    }

    /// Block until the next decoded event is available.
    ///
    /// Returns `Err` once the source is closed (EOF) or a fatal read
    /// error occurred.
    pub fn read(&self) -> io::Result<Event> {
        block_on_parking(|task| self.poll_event(task)).unwrap_or_else(|| Err(closed_err()))
    }

    /// Issue `query` (a single query, or a tuple/array batch): write its
    /// request(s) to `out`, flush once, and return the in-flight reply
    /// handle(s) without blocking.
    ///
    /// The request is written synchronously, so `out` is only borrowed for
    /// the duration of this call; the returned handles borrow nothing.
    /// This lets several queries run concurrently — submit each, then
    /// collect them in any order. Replies may arrive in any order;
    /// non-reply events stay queued for [`read`](Self::read), and each
    /// reply event is also delivered through a later read. Dropping a
    /// reply handle before it resolves cancels that query.
    ///
    /// Collect with [`QueryReply::try_take`](super::query::QueryReply::try_take),
    /// the blocking [`query_blocking`](Self::query_blocking), or — under
    /// the `async` feature — by `.await`ing the handle.
    pub fn query<Q, W>(&self, out: &mut W, query: Q, timeout: Duration) -> io::Result<Q::Replies>
    where
        Q: Query,
        W: Write,
    {
        let deadline = Instant::now() + timeout;
        let replies = {
            let mut src = self.source.lock().unwrap();
            let writer: &mut dyn Write = &mut *out;
            let mut reg = Registrar::new(writer, src.observers_mut(), deadline);
            query.issue(&mut reg)?
        };
        out.flush()?;
        // Interrupt the reader thread's readiness wait so it recomputes
        // its timeout to honour the new query's deadline.
        let _ = self.waker.wake();
        Ok(replies)
    }

    /// Issue `query` and block up to `timeout`, parking until every reply
    /// has arrived (or its deadline elapsed), then return the collected
    /// value(s).
    ///
    /// Convenience for [`query`](Self::query) followed by collecting each
    /// reply. A single query yields `Option<T>` (`None` on timeout); a
    /// batch yields a tuple/array of each member's own. Several threads
    /// sharing one stream may call this concurrently.
    pub fn query_blocking<Q, W>(
        &self,
        out: &mut W,
        query: Q,
        timeout: Duration,
    ) -> io::Result<Q::Resolved>
    where
        Q: Query,
        W: Write,
    {
        let replies = self.query(out, query, timeout)?;
        let waker = TaskWaker::from(Arc::new(ThreadWaker(std::thread::current())));
        loop {
            if Q::ready(&replies) {
                break;
            }
            Q::arm(&replies, &waker);
            // Re-check after arming: the reader thread may have resolved a
            // slot in between, and its wake would otherwise be lost.
            if Q::ready(&replies) {
                break;
            }
            match Q::deadline(&replies) {
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        break;
                    }
                    std::thread::park_timeout(d - now);
                }
                None => break,
            }
        }
        Ok(Q::resolve(replies))
    }

    /// Whether the backing source delivers [`Event::Resize`] from the
    /// kernel's out-of-band window-resize notification (`SIGWINCH` on
    /// Unix). See [`EventSource::set_handle_resize`].
    pub fn handle_resize(&self) -> bool {
        self.source.lock().unwrap().handle_resize()
    }

    /// Control whether the backing source delivers [`Event::Resize`] from
    /// the kernel's out-of-band window-resize notification (`SIGWINCH` on
    /// Unix). See [`EventSource::set_handle_resize`].
    pub fn set_handle_resize(&self, enable: bool) {
        self.source.lock().unwrap().set_handle_resize(enable);
    }

    /// The shared source, for callers that built the stream with
    /// [`from_shared`](Self::from_shared) and kept their own clone.
    pub fn shared_source(&self) -> Arc<Mutex<EventSource<I>>> {
        Arc::clone(&self.source)
    }

    /// Poll for the next event, arming `task` to be woken when one
    /// arrives. Drives both the blocking [`read`](Self::read) and the
    /// asynchronous [`Stream`] face.
    pub(super) fn poll_event(&self, task: &TaskWaker) -> Poll<Option<io::Result<Event>>> {
        if let Some(ev) = self.source.lock().unwrap().try_read() {
            return Poll::Ready(Some(Ok(ev)));
        }
        // Arm the waker, then re-check the queue: the reader thread may
        // have ingested between the check above and arming here, and its
        // wake would otherwise be lost.
        {
            let mut inner = self.shared.inner.lock().unwrap();
            if let Some(e) = inner.error.take() {
                return Poll::Ready(Some(Err(e)));
            }
            if inner.closed {
                return Poll::Ready(None);
            }
            inner.task = Some(task.clone());
        }
        if let Some(ev) = self.source.lock().unwrap().try_read() {
            return Poll::Ready(Some(Ok(ev)));
        }
        Poll::Pending
    }
}

impl<I: Input> Drop for EventStream<I> {
    fn drop(&mut self) {
        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.closed = true;
        }
        // Unblock the reader thread so it observes `closed` and exits.
        let _ = self.waker.wake();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(feature = "async")]
impl<I: Input> futures_core::Stream for EventStream<I> {
    type Item = io::Result<Event>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.poll_event(cx.waker())
    }
}

/// Wakes a parked thread by unparking it; the synchronous counterpart to
/// an async task waker.
struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Drive `poll` to completion on the current thread, parking between
/// wakes. The waker handed to `poll` unparks this thread, so the reader
/// thread resolving a slot wakes the parked caller.
fn block_on_parking<T>(mut poll: impl FnMut(&TaskWaker) -> Poll<T>) -> T {
    let waker = TaskWaker::from(Arc::new(ThreadWaker(std::thread::current())));
    loop {
        match poll(&waker) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

fn closed_err() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "event reader closed")
}

fn fail(shared: &Shared, e: io::Error) {
    let mut inner = shared.inner.lock().unwrap();
    inner.error = Some(e);
    inner.closed = true;
    wake_task(&mut inner);
}

fn reader_loop<I: Input>(
    source: Arc<Mutex<EventSource<I>>>,
    poller: Arc<dyn Poller>,
    slots: usize,
    shared: Arc<Shared>,
) {
    let mut scratch = vec![false; slots];
    loop {
        // 1. Shutdown takes priority.
        if shared.inner.lock().unwrap().closed {
            return;
        }

        // 2. Wait the shorter of the source's decode deadlines and the
        //    nearest query deadline, routing a timeout to the right
        //    expiry. A query deadline must not expire the source's
        //    partial-sequence state, so it carries `DeadlineKind::None`.
        let (src_timeout, src_kind, query_deadline) = {
            let s = source.lock().unwrap();
            let (t, k) = s.next_timeout();
            (t, k, s.observers_nearest_deadline())
        };
        let (timeout, kind) = combine_timeout(src_timeout, src_kind, query_deadline);

        // 3. Block for readiness without holding the source lock, so a
        //    consumer can take the lock meanwhile.
        let n = match poller.poll(&mut scratch, timeout) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                fail(&shared, e);
                return;
            }
        };

        // 4. Take the lock and drain (re-checking readiness) or expire.
        //    Decoded events offer themselves to the query observers as
        //    they are produced (see `EventSource::emit`), so query slots
        //    fill here; resolve any whose deadline has now passed.
        let mut s = source.lock().unwrap();
        if n == 0 {
            s.expire(kind);
        } else if let Err(e) = s.drain_after_wait()
            && e.kind() != io::ErrorKind::Interrupted
        {
            drop(s);
            fail(&shared, e);
            return;
        }
        s.expire_observers(Instant::now());
        let mut inner = shared.inner.lock().unwrap();
        if s.has_events() {
            wake_task(&mut inner);
        }
    }
}

/// Pick the sooner of the source's decode deadline and the nearest query
/// deadline. The returned [`DeadlineKind`] is the source's only when the
/// source deadline governs, so a query-driven wakeup never expires the
/// source's buffered partial sequence early.
fn combine_timeout(
    src_timeout: Option<Duration>,
    src_kind: DeadlineKind,
    query_deadline: Option<Instant>,
) -> (Option<Duration>, DeadlineKind) {
    let query_remaining = query_deadline.map(|d| d.saturating_duration_since(Instant::now()));
    match (src_timeout, query_remaining) {
        (None, None) => (None, DeadlineKind::None),
        (Some(s), None) => (Some(s), src_kind),
        (None, Some(q)) => (Some(q), DeadlineKind::None),
        (Some(s), Some(q)) => {
            if s <= q {
                (Some(s), src_kind)
            } else {
                (Some(q), DeadlineKind::None)
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::os::unix::io::AsRawFd;
    use std::sync::Arc;

    use super::*;
    use crate::event::query::Single;
    use crate::event::{Key, KeyCode};

    fn make_pipe() -> (File, File) {
        let mut fds = [0i32; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "pipe(2) failed");
        // SAFETY: pipe(2) just produced two fresh, owned fds.
        let rx = unsafe { File::from_raw_fd(fds[0]) };
        let tx = unsafe { File::from_raw_fd(fds[1]) };
        (rx, tx)
    }

    fn write_bytes(f: &File, bytes: &[u8]) {
        let n = unsafe { libc::write(f.as_raw_fd(), bytes.as_ptr() as *const _, bytes.len()) };
        assert_eq!(n, bytes.len() as isize);
    }

    /// Query resolving on `KeyPress('b')`, for pipe-driven tests without a
    /// real terminal reply.
    const KEY_B: Single<char> = Single::new(b"REQ", reply_key_b);
    fn reply_key_b(ev: &Event) -> Option<char> {
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('b'),
                ..
            }) => Some('b'),
            _ => None,
        }
    }

    /// Companion query resolving on `KeyPress('c')`.
    const KEY_C: Single<char> = Single::new(b"REQ2", reply_key_c);
    fn reply_key_c(ev: &Event) -> Option<char> {
        match ev {
            Event::KeyPress(Key {
                code: KeyCode::Char('c'),
                ..
            }) => Some('c'),
            _ => None,
        }
    }

    #[test]
    fn reads_events_in_order() {
        let (rx, tx) = make_pipe();
        let stream = EventSource::new(rx).unwrap().into_stream();
        write_bytes(&tx, b"ab");
        let a = stream.read().unwrap();
        let b = stream.read().unwrap();
        assert!(matches!(a, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
        assert!(matches!(b, Event::KeyPress(k) if k.code == KeyCode::Char('b')));
    }

    #[test]
    fn blocking_query_resolves_reply_keeping_input_visible() {
        let (rx, tx) = make_pipe();
        let stream = EventSource::new(rx).unwrap().into_stream();
        // A user keypress 'a' arrives before the awaited reply 'b'.
        write_bytes(&tx, b"ab");
        let mut out: Vec<u8> = Vec::new();
        let got = stream
            .query_blocking(&mut out, KEY_B, Duration::from_secs(1))
            .unwrap();
        assert_eq!(got, Some('b'));
        assert_eq!(out, b"REQ");
        // Both 'a' and the reply 'b' are still delivered, in order.
        let a = stream.read().unwrap();
        assert!(matches!(a, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
        let b = stream.read().unwrap();
        assert!(matches!(b, Event::KeyPress(k) if k.code == KeyCode::Char('b')));
    }

    #[test]
    fn blocking_query_times_out_without_reply() {
        let (rx, tx) = make_pipe();
        let stream = EventSource::new(rx).unwrap().into_stream();
        write_bytes(&tx, b"a");
        let mut out: Vec<u8> = Vec::new();
        let got = stream
            .query_blocking(&mut out, KEY_B, Duration::from_millis(30))
            .unwrap();
        assert_eq!(got, None);
        let ev = stream.read().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
    }

    // Submit several queries up front, then collect them: each fills its
    // own slot regardless of arrival order — the sync "fire several,
    // collect later" pattern, no async feature required.
    #[test]
    fn submit_many_collect_later() {
        let (rx, tx) = make_pipe();
        let stream = EventSource::new(rx).unwrap().into_stream();
        let mut out: Vec<u8> = Vec::new();
        let mut qb = stream
            .query(&mut out, KEY_B, Duration::from_secs(1))
            .unwrap();
        let mut qc = stream
            .query(&mut out, KEY_C, Duration::from_secs(1))
            .unwrap();
        assert_eq!(out, b"REQREQ2");
        // Replies arrive out of submission order, interleaved with input.
        write_bytes(&tx, b"cab");
        // Drain events so the reader thread fills the slots.
        let _ = stream.read().unwrap();
        let _ = stream.read().unwrap();
        let _ = stream.read().unwrap();
        assert_eq!(qb.try_take(), Some('b'));
        assert_eq!(qc.try_take(), Some('c'));
    }

    // One batched blocking call writes both requests in a single flush and
    // collects both replies independently.
    #[test]
    fn batched_blocking_query() {
        let (rx, tx) = make_pipe();
        let stream = EventSource::new(rx).unwrap().into_stream();
        let mut out: Vec<u8> = Vec::new();
        write_bytes(&tx, b"cb");
        let (b, c) = stream
            .query_blocking(&mut out, (KEY_B, KEY_C), Duration::from_secs(1))
            .unwrap();
        assert_eq!(out, b"REQREQ2");
        assert_eq!(b, Some('b'));
        assert_eq!(c, Some('c'));
    }

    // Multiple OS threads share one stream (`&EventStream` is `Send +
    // Sync`) and each blocks on its own query concurrently. Both are
    // served by the single reader thread — no async runtime, no feature.
    #[test]
    fn concurrent_queries_across_threads() {
        let (rx, tx) = make_pipe();
        let stream = Arc::new(EventSource::new(rx).unwrap().into_stream());
        let r_b = Arc::clone(&stream);
        let tb = std::thread::spawn(move || {
            let mut out: Vec<u8> = Vec::new();
            r_b.query_blocking(&mut out, KEY_B, Duration::from_secs(2))
                .unwrap()
        });
        let r_c = Arc::clone(&stream);
        let tc = std::thread::spawn(move || {
            let mut out: Vec<u8> = Vec::new();
            r_c.query_blocking(&mut out, KEY_C, Duration::from_secs(2))
                .unwrap()
        });
        std::thread::sleep(Duration::from_millis(20));
        write_bytes(&tx, b"cb");
        assert_eq!(tb.join().unwrap(), Some('b'));
        assert_eq!(tc.join().unwrap(), Some('c'));
    }

    fn _assert_send_sync<T: Send + Sync>() {}
    #[test]
    fn stream_is_send_sync() {
        _assert_send_sync::<EventStream<File>>();
    }
}

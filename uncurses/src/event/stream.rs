//! Asynchronous event streaming (`async` feature).
//!
//! [`EventStream`] turns a blocking [`EventSource`] into a
//! [`futures_core::Stream`] of decoded events. There is no reactor to
//! register the terminal handle with, so a dedicated reader thread does
//! the blocking readiness waits; decoded events flow through the source's
//! own queue and the consumer is woken through a single stored task
//! waker (a hand-rolled bridge — no executor or channel crate, only
//! `futures-core` for the [`Stream`] trait).
//!
//! The source is **shared**, not moved: the stream holds
//! `Arc<Mutex<EventSource>>` and the reader thread holds a clone, so a
//! synchronous caller (e.g. a `ratatui` backend's cursor-position probe)
//! can still lock the same source and run a [`query`](EventSource::query)
//! while the stream is live. The reader waits for readiness lock-free
//! (through a cloned [`Poller`], which polls by shared reference) and only
//! takes the source lock to drain what is ready — re-checking readiness
//! under the lock so a blocking input fd never stalls the lock even if a
//! concurrent query consumed the bytes first.

use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker as TaskWaker};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures_core::Stream;

use super::Event;
use super::poll::Poller;
use super::query::Query;
use super::source::{EventSource, Input, Waker};

/// Type-erased reply matcher dispatched to the reader thread.
type Predicate = Box<dyn Fn(&Event) -> bool + Send>;

/// A query handed from the async side to the reader thread.
struct QueryRequest {
    predicate: Predicate,
    timeout: Duration,
}

/// Coordination state between the reader thread and the consumer.
///
/// Decoded events live in the source's own queue, not here; this only
/// carries the consumer waker, the terminal error/closed signal, and the
/// query hand-off. `query` and `next` are never awaited concurrently
/// (both take `&mut self`), so a single `task` slot serves both.
#[derive(Default)]
struct Inner {
    /// Consumer task waker, stored while the consumer is parked.
    task: Option<TaskWaker>,
    /// A fatal reader error, surfaced once to the consumer.
    error: Option<io::Error>,
    /// Set once the source is closed (EOF/error) or the stream is dropped.
    closed: bool,
    /// Pending query for the reader thread to run.
    query: Option<QueryRequest>,
    /// Result of the most recent query, for the awaiting consumer.
    query_result: Option<io::Result<Option<Event>>>,
}

struct Shared {
    inner: Mutex<Inner>,
}

fn wake_task(inner: &mut Inner) {
    if let Some(w) = inner.task.take() {
        w.wake();
    }
}

/// A [`Stream`] of decoded terminal events, backed by a reader thread
/// over a shared [`EventSource`].
///
/// Build one with [`EventSource::into_stream`] (standalone) or
/// [`EventStream::from_shared`] (sharing a source kept elsewhere, e.g. by
/// a backend that also needs synchronous queries). Dropping the stream
/// stops and joins the reader thread; the source itself lives as long as
/// any `Arc` to it remains.
pub struct EventStream<I: Input> {
    source: Arc<Mutex<EventSource<I>>>,
    shared: Arc<Shared>,
    /// Wakes the reader thread out of its lock-free readiness wait (for a
    /// pending query or for shutdown).
    waker: Waker,
    handle: Option<JoinHandle<()>>,
}

impl<I> EventSource<I>
where
    I: Input + 'static,
{
    /// Convert this source into an asynchronous [`EventStream`].
    ///
    /// The source is wrapped in `Arc<Mutex<_>>` and shared with a reader
    /// thread; the returned stream yields `io::Result<Event>`. To keep a
    /// handle to the source for synchronous queries, build the
    /// `Arc<Mutex<_>>` yourself and use [`EventStream::from_shared`].
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

    /// Run `query`: write its request to `out`, then await the reply, up
    /// to `timeout`.
    ///
    /// Events that are not the reply stay queued, in order, for a later
    /// poll of the stream. Returns `Ok(None)` if the terminal does not
    /// reply within `timeout`.
    pub async fn query<W, T>(
        &mut self,
        out: &mut W,
        query: &Query<T>,
        timeout: Duration,
    ) -> io::Result<Option<T>>
    where
        W: Write,
        T: 'static,
    {
        out.write_all(query.request())?;
        out.flush()?;

        // Capture the matcher as a plain `fn` pointer so the predicate
        // closure is `Send + 'static` without borrowing `query`.
        let reply = query.reply_fn();
        let predicate: Predicate = Box::new(move |ev| reply(ev).is_some());

        {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.closed {
                return Err(closed_err());
            }
            inner.query = Some(QueryRequest { predicate, timeout });
        }
        // Interrupt the reader thread's readiness wait so it picks up the
        // pending query.
        let _ = self.waker.wake();

        let ev = QueryReply {
            shared: &self.shared,
        }
        .await?;
        Ok(ev.and_then(|e| reply(&e)))
    }

    /// Whether the backing source delivers [`Event::Resize`] from the
    /// kernel's out-of-band window-resize notification (`SIGWINCH` on
    /// Unix). See [`EventSource::set_handle_resize`].
    pub fn handle_resize(&self) -> bool {
        self.source.lock().unwrap().handle_resize()
    }

    /// Control whether the backing source delivers [`Event::Resize`]
    /// from the kernel's out-of-band window-resize notification
    /// (`SIGWINCH` on Unix). Set to `false` after enabling in-band
    /// resize reports (DEC mode 2048) to avoid duplicate resize events.
    /// See [`EventSource::set_handle_resize`].
    pub fn set_handle_resize(&self, enable: bool) {
        self.source.lock().unwrap().set_handle_resize(enable);
    }
}

impl<I: Input> Stream for EventStream<I> {
    type Item = io::Result<Event>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Events live in the shared source's queue.
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
            inner.task = Some(cx.waker().clone());
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

/// Future that resolves when the reader thread posts a query result.
struct QueryReply<'a> {
    shared: &'a Shared,
}

impl Future for QueryReply<'_> {
    type Output = io::Result<Option<Event>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.shared.inner.lock().unwrap();
        if let Some(result) = inner.query_result.take() {
            return Poll::Ready(result);
        }
        if inner.closed {
            return Poll::Ready(Err(closed_err()));
        }
        inner.task = Some(cx.waker().clone());
        Poll::Pending
    }
}

fn closed_err() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "event stream closed")
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
        // 1. Shutdown or a pending query takes priority.
        let job = {
            let mut inner = shared.inner.lock().unwrap();
            if inner.closed {
                return;
            }
            inner.query.take()
        };
        if let Some(job) = job {
            let result = run_query(&source, &job, &shared);
            let mut inner = shared.inner.lock().unwrap();
            inner.query_result = Some(result);
            wake_task(&mut inner);
            continue;
        }

        // 2. Decide how long to wait (reads the source's decode deadlines).
        let (timeout, kind) = {
            let s = source.lock().unwrap();
            s.next_timeout()
        };

        // 3. Block for readiness without holding the source lock, so a
        //    synchronous query can take the lock meanwhile.
        let n = match poller.poll(&mut scratch, timeout) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                fail(&shared, e);
                return;
            }
        };

        // 4. Take the lock and drain (re-checking readiness) or expire.
        let has_events = {
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
            s.has_events()
        };
        if has_events {
            let mut inner = shared.inner.lock().unwrap();
            wake_task(&mut inner);
        }
    }
}

fn run_query<I: Input>(
    source: &Arc<Mutex<EventSource<I>>>,
    job: &QueryRequest,
    shared: &Shared,
) -> io::Result<Option<Event>> {
    let deadline = Instant::now() + job.timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let matched = {
            let mut s = source.lock().unwrap();
            s.read_matching(|ev| (job.predicate)(ev), Some(remaining))?
        };
        if let Some(ev) = matched {
            return Ok(Some(ev));
        }
        // `read_matching` returns `None` on a real timeout or a spurious
        // wake (the query-submission wake, or shutdown). Retry until the
        // deadline; non-matching events stay queued in the source.
        if shared.inner.lock().unwrap().closed || Instant::now() >= deadline {
            return Ok(None);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs::File;
    use std::future::poll_fn;
    use std::os::fd::FromRawFd;
    use std::os::unix::io::AsRawFd;
    use std::sync::Arc;
    use std::task::Wake;

    use super::*;
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

    /// Minimal executor: poll the future, parking the thread between
    /// wakes. Keeps the async tests free of any runtime dependency.
    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    fn block_on<F: Future>(mut fut: F) -> F::Output {
        let waker = TaskWaker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        // SAFETY: `fut` lives on this stack frame and is never moved after
        // being pinned here.
        let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(v) => return v,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    async fn next<I: Input>(stream: &mut EventStream<I>) -> Option<io::Result<Event>> {
        poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await
    }

    /// Query that resolves on `KeyPress('b')`, for pipe-driven tests
    /// without a real terminal reply.
    const KEY_B: Query<()> = Query::new(b"REQ", reply_key_b);
    fn reply_key_b(ev: &Event) -> Option<()> {
        matches!(
            ev,
            Event::KeyPress(Key {
                code: KeyCode::Char('b'),
                ..
            })
        )
        .then_some(())
    }

    #[test]
    fn stream_yields_events_in_order() {
        let (rx, tx) = make_pipe();
        let mut stream = EventSource::new(rx).unwrap().into_stream();
        write_bytes(&tx, b"ab");
        let a = block_on(next(&mut stream)).unwrap().unwrap();
        let b = block_on(next(&mut stream)).unwrap().unwrap();
        assert!(matches!(a, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
        assert!(matches!(b, Event::KeyPress(k) if k.code == KeyCode::Char('b')));
    }

    #[test]
    fn stream_query_plucks_reply_leaving_others() {
        let (rx, tx) = make_pipe();
        let mut stream = EventSource::new(rx).unwrap().into_stream();
        // A user keypress 'a' arrives before the awaited reply 'b'.
        write_bytes(&tx, b"ab");
        let mut out: Vec<u8> = Vec::new();
        let got = block_on(stream.query(&mut out, &KEY_B, Duration::from_secs(1))).unwrap();
        assert!(got.is_some(), "reply should be found");
        assert_eq!(out, b"REQ");
        // The non-matching 'a' is still delivered, in order, by the stream.
        let ev = block_on(next(&mut stream)).unwrap().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
    }

    #[test]
    fn stream_query_times_out_without_reply() {
        let (rx, tx) = make_pipe();
        let mut stream = EventSource::new(rx).unwrap().into_stream();
        write_bytes(&tx, b"a");
        let mut out: Vec<u8> = Vec::new();
        let got = block_on(stream.query(&mut out, &KEY_B, Duration::from_millis(30))).unwrap();
        assert!(got.is_none());
        // The user input survives the failed query.
        let ev = block_on(next(&mut stream)).unwrap().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
    }

    // Path A: a synchronous query on the shared source runs while the
    // stream's reader thread is live (the get_cursor_position scenario).
    // The query locks the same source the reader uses; whichever side
    // reads the reply byte first, read_matching finds it (queue or fd),
    // and unrelated input is still delivered by the stream afterward.
    #[test]
    fn shared_source_sync_query_while_streaming() {
        let (rx, tx) = make_pipe();
        let source = Arc::new(Mutex::new(EventSource::new(rx).unwrap()));
        let mut stream = EventStream::from_shared(Arc::clone(&source));

        // The awaited reply 'b' arrives on the shared input.
        write_bytes(&tx, b"b");
        let mut out: Vec<u8> = Vec::new();
        let got = {
            let mut s = source.lock().unwrap();
            s.query(&mut out, &KEY_B, Duration::from_secs(1)).unwrap()
        };
        assert!(got.is_some(), "sync query should find the reply");
        assert_eq!(out, b"REQ");

        // A later user event still flows through the stream.
        write_bytes(&tx, b"a");
        let ev = block_on(next(&mut stream)).unwrap().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
    }
}

//! Asynchronous event streaming (`async` feature).
//!
//! [`EventStream`] turns a blocking [`EventSource`] into a
//! [`futures_core::Stream`] of decoded events. Because there is no reactor
//! to register the terminal handle with, the source is moved onto a
//! dedicated reader thread; the thread does the blocking reads and hands
//! events to the async side through a small lock-protected queue plus a
//! single stored task waker (a hand-rolled bridge — no executor or channel
//! crate is pulled in, only `futures-core` for the [`Stream`] trait).
//!
//! Queries keep working after the switch to async: [`EventStream::query`]
//! dispatches the same [`read_matching`](EventSource::read_matching) the
//! synchronous path uses onto the reader thread (where the timeout is
//! enforced by the source's own poller — no async timer needed) and awaits
//! the reply.

use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker as TaskWaker};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures_core::Stream;

use super::Event;
use super::query::Query;
use super::source::{EventSource, Input, Waker};

/// Type-erased reply matcher dispatched to the reader thread. The matcher
/// itself is a function pointer; only the per-call closure that drops the
/// result type is boxed.
type Predicate = Box<dyn Fn(&Event) -> bool + Send>;

/// A query handed from the async side to the reader thread.
struct QueryRequest {
    predicate: Predicate,
    timeout: Duration,
}

/// State shared between the reader thread and the [`EventStream`].
///
/// `query` and `next` are never awaited concurrently (both take
/// `&mut self`), so a single `task` waker slot serves both.
#[derive(Default)]
struct Inner {
    /// Decoded events waiting to be yielded by the stream.
    events: std::collections::VecDeque<io::Result<Event>>,
    /// Consumer task waker, stored while the consumer is parked.
    task: Option<TaskWaker>,
    /// Pending query for the reader thread to run.
    query: Option<QueryRequest>,
    /// Result of the most recent query, for the awaiting consumer.
    query_result: Option<io::Result<Option<Event>>>,
    /// Set once the source is closed (EOF/error) or the stream is dropped.
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

/// A [`Stream`] of decoded terminal events, backed by a reader thread.
///
/// Build one with [`EventSource::into_stream`]. Dropping the stream stops
/// and joins the reader thread.
pub struct EventStream {
    shared: Arc<Shared>,
    /// Wakes the reader thread out of its blocking read (for a pending
    /// query or for shutdown).
    waker: Waker,
    handle: Option<JoinHandle<()>>,
}

impl<I> EventSource<I>
where
    I: Input + 'static,
{
    /// Convert this source into an asynchronous [`EventStream`].
    ///
    /// The source moves onto a dedicated reader thread; the returned
    /// stream yields `io::Result<Event>`. Run any synchronous
    /// [`query`](EventSource::query) calls before converting — or use
    /// [`EventStream::query`] afterwards.
    pub fn into_stream(self) -> EventStream {
        EventStream::spawn(self)
    }
}

impl EventStream {
    fn spawn<I>(source: EventSource<I>) -> Self
    where
        I: Input + 'static,
    {
        let waker = source.waker();
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
        });
        let thread_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("uncurses-event-reader".to_string())
            .spawn(move || reader_loop(source, thread_shared))
            .expect("spawn event reader thread");
        Self {
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
        // Interrupt the reader thread's blocking read so it picks up the
        // pending query.
        let _ = self.waker.wake();

        let ev = QueryReply {
            shared: &self.shared,
        }
        .await?;
        Ok(ev.and_then(|e| reply(&e)))
    }
}

impl Stream for EventStream {
    type Item = io::Result<Event>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut inner = self.shared.inner.lock().unwrap();
        if let Some(item) = inner.events.pop_front() {
            return Poll::Ready(Some(item));
        }
        if inner.closed {
            return Poll::Ready(None);
        }
        inner.task = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for EventStream {
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

fn reader_loop<I: Input>(mut source: EventSource<I>, shared: Arc<Shared>) {
    loop {
        let job = {
            let mut inner = shared.inner.lock().unwrap();
            if inner.closed {
                return;
            }
            inner.query.take()
        };

        if let Some(job) = job {
            let result = run_query(&mut source, &job, &shared);
            let mut inner = shared.inner.lock().unwrap();
            inner.query_result = Some(result);
            wake_task(&mut inner);
            continue;
        }

        match source.read() {
            Ok(ev) => {
                let mut inner = shared.inner.lock().unwrap();
                inner.events.push_back(Ok(ev));
                wake_task(&mut inner);
            }
            // Woken for a pending query or for shutdown; loop to re-check.
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                let mut inner = shared.inner.lock().unwrap();
                inner.events.push_back(Err(e));
                inner.closed = true;
                wake_task(&mut inner);
                return;
            }
        }
    }
}

fn run_query<I: Input>(
    source: &mut EventSource<I>,
    job: &QueryRequest,
    shared: &Shared,
) -> io::Result<Option<Event>> {
    let deadline = Instant::now() + job.timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Some(ev) = source.read_matching(|ev| (job.predicate)(ev), Some(remaining))? {
            return Ok(Some(ev));
        }
        // `read_matching` returns `None` on a real timeout or on a
        // spurious wake (the query-submission wake, or shutdown). Retry
        // until the deadline; non-matching events stay queued in the
        // source for later delivery.
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

    async fn next(stream: &mut EventStream) -> Option<io::Result<Event>> {
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
}

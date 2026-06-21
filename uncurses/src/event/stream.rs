//! Thread-backed asynchronous stream over a shared [`EventSource`].
//!
//! ## Purpose
//!
//! [`EventStream`] adapts a blocking [`EventSource`] into a
//! [`futures_core::Stream`] of `io::Result<Event>`. It preserves the same
//! decoder and queue semantics as synchronous reading while avoiding blocking
//! the async task that polls the stream.
//!
//! ```text
//! async task poll_next ─┬─ try lock + drain ready events ──▶ Poll::Ready
//!                       └─ arm helper thread ──▶ source.poll(None) ──▶ wake task
//! drop stream ──▶ source Waker ──▶ helper exits
//! ```
//!
//! ## Key types
//!
//! * [`EventStream`] owns or shares an `Arc<Mutex<EventSource<_>>>`, a source
//!   [`Waker`], and a helper thread channel.
//! * A private `Wait` value tells the helper whether a wait is in flight and
//!   whether stream drop requested shutdown.
//!
//! ## Lifecycle
//!
//! Use [`EventSource::into_stream`] when the stream should be the sole owner of
//! the source. Use [`EventStream::from_shared`] when synchronous code keeps an
//! `Arc<Mutex<_>>` clone. Dropping the stream wakes the helper so it can stop;
//! the underlying shared source is not closed or drained.
//!
//! ## Coexistence caveats
//!
//! Sharing one source between a live stream and synchronous readers is
//! supported but best-effort. The helper holds the source lock while parked in
//! readiness wait, events go to whichever consumer drains first, and events are
//! not broadcast. Read errors surface once as `Some(Err(_))`; after that the
//! stream fuses to `None`.
//!
//! [`futures_core::Stream`]: https://docs.rs/futures-core/latest/futures_core/stream/trait.Stream.html
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, TryLockError};
use std::task::{Context, Poll, Waker as TaskWaker};
use std::time::Duration;

use super::Event;
use super::source::{EventSource, Input, Waker};

/// A readiness wait requested by the polling task: block until the source
/// signals input (or a decode deadline elapses), then wake the task's
/// latest registered waker.
struct Wait {
    /// Cleared by the helper just before waking, so the task can request a
    /// fresh wait on its next poll. Guards against queueing more than one
    /// wait at a time.
    dispatched: Arc<AtomicBool>,
    /// Set on drop to break the helper out of its blocking wait and end the
    /// thread instead of re-arming.
    shutdown: Arc<AtomicBool>,
}

/// A thread-backed [`futures_core::Stream`] of `io::Result<Event>` over a
/// shared [`EventSource`].
///
/// Build one with [`EventSource::into_stream`] (sole owner) or
/// [`EventStream::from_shared`] (sharing a source kept elsewhere). A helper
/// thread blocks in [`EventSource::poll`], which reads and decodes input
/// into the source's queue, then wakes the polling task; the task drains
/// queued events (and may itself decode via a non-blocking poll). The stream
/// yields `Some(Ok(event))` per decoded event and, once, `Some(Err(_))` on a
/// read error or end-of-input, then fuses to `None`. Dropping it ends the
/// helper thread; the shared source is left intact for any other holder.
pub struct EventStream<I: Input> {
    source: Arc<Mutex<EventSource<I>>>,
    /// Cloned source waker, used on drop to break the helper's blocking
    /// wait.
    waker: Waker,
    /// Set while a readiness wait is queued, so only one is in flight.
    dispatched: Arc<AtomicBool>,
    /// Set on drop to tell the in-flight wait (if any) to end the helper.
    shutdown: Arc<AtomicBool>,
    /// Latest task waker, refreshed on every pending poll so the helper
    /// always wakes the current one even if a wait is already in flight
    /// (the task's waker may change between polls).
    task_waker: Arc<Mutex<Option<TaskWaker>>>,
    /// Hands a readiness wait to the helper thread.
    waits: SyncSender<Wait>,
    /// Latched once a read error or end-of-input has been surfaced, after
    /// which the stream yields `None`.
    done: bool,
}

impl<I> EventSource<I>
where
    I: Input + 'static,
{
    /// Convert this source into a thread-backed [`EventStream`].
    ///
    /// The source is wrapped in `Arc<Mutex<_>>` owned solely by the stream.
    /// To keep reading the source synchronously alongside the stream, build
    /// the `Arc<Mutex<_>>` yourself and use [`EventStream::from_shared`].
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
    /// The caller may keep its own clone of the `Arc` to read the source
    /// synchronously while the stream is live (see the coexistence caveats
    /// on this module).
    pub fn from_shared(source: Arc<Mutex<EventSource<I>>>) -> Self {
        let waker = source.lock().unwrap().waker();
        let (waits, rx) = mpsc::sync_channel::<Wait>(1);
        let task_waker: Arc<Mutex<Option<TaskWaker>>> = Arc::new(Mutex::new(None));
        let wait_source = Arc::clone(&source);
        let wait_waker = Arc::clone(&task_waker);
        std::thread::Builder::new()
            .name("uncurses-event-waiter".to_string())
            .spawn(move || waiter_loop(wait_source, wait_waker, rx))
            .expect("spawn event waiter thread");
        Self {
            source,
            waker,
            dispatched: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
            task_waker,
            waits,
            done: false,
        }
    }
}

impl<I> EventStream<I>
where
    I: Input,
{
    /// Record the current task waker and, if none is in flight, queue a
    /// single readiness wait so the helper wakes the task when input
    /// arrives or a decode deadline elapses.
    fn arm(&self, cx: &Context<'_>) {
        // Always refresh the waker, even when a wait is already in flight:
        // the helper wakes whatever is latest, so a changed waker is never
        // lost.
        *self.task_waker.lock().unwrap() = Some(cx.waker().clone());
        if !self
            .dispatched
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .unwrap_or_else(|prev| prev)
        {
            self.shutdown.store(false, Ordering::SeqCst);
            let _ = self.waits.send(Wait {
                dispatched: Arc::clone(&self.dispatched),
                shutdown: Arc::clone(&self.shutdown),
            });
        }
    }
}

impl<I: Input> Drop for EventStream<I> {
    fn drop(&mut self) {
        // Tell the in-flight wait to end the helper, then break it out of
        // its blocking readiness wait. The `waits` channel closes as this
        // value drops, so the helper's `recv` then returns and the thread
        // exits. The shared source itself is untouched.
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = self.waker.wake();
    }
}

impl<I: Input> futures_core::Stream for EventStream<I> {
    type Item = io::Result<Event>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        // Try to decode on this task without blocking. If the helper holds
        // the lock (parked in its blocking wait), fall through to arm a wait
        // and stay pending rather than block here.
        match this.source.try_lock() {
            Ok(mut src) => {
                if let Some(ev) = src.try_read() {
                    return Poll::Ready(Some(Ok(ev)));
                }
                match src.poll(Some(Duration::ZERO)) {
                    Ok(_) => {
                        if let Some(ev) = src.try_read() {
                            return Poll::Ready(Some(Ok(ev)));
                        }
                    }
                    // A stray wake is not an error; just stay pending.
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    // A read error (including end-of-input) surfaces once,
                    // then the stream fuses.
                    Err(e) => {
                        this.done = true;
                        return Poll::Ready(Some(Err(e)));
                    }
                }
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Poisoned(_)) => {
                this.done = true;
                return Poll::Ready(Some(Err(io::Error::other("event source mutex poisoned"))));
            }
        }
        this.arm(cx);
        Poll::Pending
    }
}

fn waiter_loop<I: Input>(
    source: Arc<Mutex<EventSource<I>>>,
    task_waker: Arc<Mutex<Option<TaskWaker>>>,
    waits: Receiver<Wait>,
) {
    while let Ok(wait) = waits.recv() {
        // Block (holding the source lock only while waiting, as a blocking
        // read would) until input is ready, a decode deadline elapses, or a
        // wake fires. Loop past spurious wakes unless asked to shut down.
        loop {
            // Re-check shutdown before each blocking wait so a drop that
            // raced the wait is observed promptly.
            if wait.shutdown.load(Ordering::SeqCst) {
                break;
            }
            let outcome = source.lock().unwrap().poll(None);
            match outcome {
                // Input ready / deadline produced an event: wake the task.
                Ok(true) => break,
                // A read error: let the task observe it on its next poll.
                Err(_) => break,
                // A wake with nothing to show: stop only if shutting down.
                Ok(false) => {
                    if wait.shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
        wait.dispatched.store(false, Ordering::SeqCst);
        if let Some(waker) = task_waker.lock().unwrap().as_ref() {
            waker.wake_by_ref();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::os::unix::io::AsRawFd;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Wake, Waker};
    use std::time::Duration;

    use futures_core::Stream;

    use super::*;
    use crate::event::KeyCode;

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

    /// Minimal waker that flips a flag when woken, so a parked test thread
    /// can re-poll the stream without an async runtime.
    struct FlagWaker(AtomicBool);

    impl Wake for FlagWaker {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// Drive `poll_next` to its next `Ready`, parking briefly between
    /// pending polls.
    fn next_blocking<I: Input>(stream: &mut EventStream<I>) -> Option<io::Result<Event>> {
        let flag = Arc::new(FlagWaker(AtomicBool::new(false)));
        let waker = Waker::from(Arc::clone(&flag));
        loop {
            let mut cx = Context::from_waker(&waker);
            match Pin::new(&mut *stream).poll_next(&mut cx) {
                Poll::Ready(item) => return item,
                Poll::Pending => {
                    while !flag.0.swap(false, Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }
    }

    #[test]
    fn reads_events_in_order() {
        let (rx, tx) = make_pipe();
        let mut stream = EventSource::new(rx).unwrap().into_stream();
        write_bytes(&tx, b"ab");
        let a = next_blocking(&mut stream).unwrap().unwrap();
        let b = next_blocking(&mut stream).unwrap().unwrap();
        assert!(matches!(a, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
        assert!(matches!(b, Event::KeyPress(k) if k.code == KeyCode::Char('b')));
    }

    #[test]
    fn waits_then_reads_late_input() {
        let (rx, tx) = make_pipe();
        let mut stream = EventSource::new(rx).unwrap().into_stream();
        // Input arrives after the first poll parks, so the wake path runs.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            write_bytes(&tx, b"z");
        });
        let z = next_blocking(&mut stream).unwrap().unwrap();
        assert!(matches!(z, Event::KeyPress(k) if k.code == KeyCode::Char('z')));
    }

    #[test]
    fn surfaces_error_then_fuses_on_input_eof() {
        let (rx, tx) = make_pipe();
        let mut stream = EventSource::new(rx).unwrap().into_stream();
        // Closing the write end makes the read end report EOF, which the
        // stream surfaces once as an error item and then fuses to `None`.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(tx);
        });
        let item = next_blocking(&mut stream).unwrap();
        assert!(
            matches!(&item, Err(e) if e.kind() == io::ErrorKind::UnexpectedEof),
            "expected UnexpectedEof, got {:?}",
            item
        );
        assert!(
            next_blocking(&mut stream).is_none(),
            "stream should fuse to None after the error"
        );
    }

    #[test]
    fn sync_reads_coexist_with_a_live_stream() {
        // A second handle to the same shared source can read synchronously
        // while a stream is live; neither path deadlocks.
        let (rx, tx) = make_pipe();
        let shared = Arc::new(Mutex::new(EventSource::new(rx).unwrap()));
        let mut stream = EventStream::from_shared(Arc::clone(&shared));
        write_bytes(&tx, b"a");
        // The synchronous side can take the lock and read.
        let ev = shared.lock().unwrap().read().unwrap();
        assert!(matches!(ev, Event::KeyPress(k) if k.code == KeyCode::Char('a')));
        // The stream still functions for subsequent input.
        write_bytes(&tx, b"b");
        let b = next_blocking(&mut stream).unwrap().unwrap();
        assert!(matches!(b, Event::KeyPress(k) if k.code == KeyCode::Char('b')));
    }

    fn _assert_send_sync<T: Send + Sync>() {}
    #[test]
    fn stream_is_send_sync() {
        _assert_send_sync::<EventStream<File>>();
    }
}

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
//!                       └─ arm helper thread ─┬─ brief lock: read decode deadline
//!                                             └─ block lock-free on private poller ──▶ wake task
//! deadline change / drop stream ──▶ private Waker ──▶ helper wakes
//! ```
//!
//! ## Key types
//!
//! * [`EventStream`] owns or shares an `Arc<Mutex<EventSource<_>>>` and gives
//!   its helper a private poller, notification [`Waker`], and request channel.
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
//! supported but best-effort. The helper waits **lock-free** on its own poller
//! (it locks the source only briefly to read the decode deadline, then releases
//! it), so a synchronous reader, [`Screen::render`], or teardown can take the
//! source lock while the stream is parked. Events go to whichever consumer
//! drains first and are not broadcast. Read errors surface once as
//! `Some(Err(_))`; after that the stream fuses to `None`.
//!
//! [`Screen::render`]: crate::screen::Screen::render
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
use super::poll::Poller;
use super::source::{EventSource, Input, READY_SLOTS, Waker};

/// A private wake handle watched only by this stream's helper.
pub(super) struct DeadlineWake {
    waker: Waker,
    task_waker: Mutex<Option<TaskWaker>>,
    error: Mutex<Option<io::Error>>,
}

impl DeadlineWake {
    pub(super) fn notify(&self) {
        if let Err(error) = self.waker.wake() {
            self.fail(error);
        }
    }

    fn fail(&self, error: io::Error) {
        self.error.lock().unwrap().get_or_insert(error);
        self.wake_task();
    }

    fn wake_task(&self) {
        if let Some(waker) = self.task_waker.lock().unwrap().as_ref() {
            waker.wake_by_ref();
        }
    }
}

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
    #[cfg(test)]
    started: Option<SyncSender<Option<Duration>>>,
}

/// A thread-backed [`futures_core::Stream`] of `io::Result<Event>` over a
/// shared [`EventSource`].
///
/// Build one with [`EventSource::into_stream`] (sole owner) or
/// [`EventStream::from_shared`] (sharing a source kept elsewhere). A helper
/// thread waits for readiness or a decode deadline, then wakes the polling
/// task. The task drains queued events and decodes through a non-blocking poll.
/// A private notification handle refreshes waits after deadline changes.
/// The stream yields `Some(Ok(event))` for each event. It reports an input or
/// readiness error once as `Some(Err(_))`, then fuses to `None`. Dropping it ends the
/// helper thread; the shared source is left intact for any other holder.
pub struct EventStream<I: Input> {
    source: Arc<Mutex<EventSource<I>>>,
    notification: Arc<DeadlineWake>,
    /// Set while a readiness wait is queued, so only one is in flight.
    dispatched: Arc<AtomicBool>,
    /// Set on drop to tell the in-flight wait (if any) to end the helper.
    shutdown: Arc<AtomicBool>,
    /// Hands a readiness wait to the helper thread.
    waits: SyncSender<Wait>,
    /// Latched once a read error or end-of-input has been surfaced, after
    /// which the stream yields `None`.
    done: bool,
    #[cfg(test)]
    wait_started: Option<SyncSender<Option<Duration>>>,
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
        let notification = Arc::new(DeadlineWake {
            waker: Waker::new().expect("create event waiter notification"),
            task_waker: Mutex::new(None),
            error: Mutex::new(None),
        });
        let poller = {
            let mut src = source.lock().unwrap();
            let poller = src
                .waiter_poller(&notification.waker)
                .expect("create event waiter poller");
            src.deadline_wakers.retain(|weak| weak.strong_count() > 0);
            src.deadline_wakers.push(Arc::downgrade(&notification));
            poller
        };
        let (waits, rx) = mpsc::sync_channel::<Wait>(1);
        let wait_source = Arc::clone(&source);
        let wait_notification = Arc::clone(&notification);
        std::thread::Builder::new()
            .name("uncurses-event-waiter".to_string())
            .spawn(move || waiter_loop(wait_source, poller, wait_notification, rx))
            .expect("spawn event waiter thread");
        Self {
            source,
            notification,
            dispatched: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
            waits,
            done: false,
            #[cfg(test)]
            wait_started: None,
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
        *self.notification.task_waker.lock().unwrap() = Some(cx.waker().clone());
        if !self
            .dispatched
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .unwrap_or_else(|prev| prev)
        {
            self.shutdown.store(false, Ordering::SeqCst);
            let _ = self.waits.send(Wait {
                dispatched: Arc::clone(&self.dispatched),
                shutdown: Arc::clone(&self.shutdown),
                #[cfg(test)]
                started: self.wait_started.clone(),
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
        self.notification.notify();
    }
}

impl<I: Input> futures_core::Stream for EventStream<I> {
    type Item = io::Result<Event>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        if let Some(error) = this.notification.error.lock().unwrap().take() {
            this.done = true;
            return Poll::Ready(Some(Err(error)));
        }
        // Try to decode on this task without blocking. If the helper holds
        // the lock (parked in its brief deadline peek), fall through to arm a
        // wait and stay pending rather than block here.
        match this.source.try_lock() {
            Ok(mut src) => {
                if let Some(ev) = src.try_read() {
                    return Poll::Ready(Some(Ok(ev)));
                }
                // Only drive I/O when no waiter is in flight. A dispatched
                // waiter owns the next readiness wait and already captured the
                // decode deadline; draining here could consume input (and set
                // a fresh ESC/paste deadline) that the parked waiter's timeout
                // won't honor, hanging a lone Esc until unrelated input. When
                // a waiter is in flight the input stays level-ready, so the
                // waiter wakes on it and the next poll (with `dispatched`
                // cleared) drains and re-arms with the new deadline.
                if !this.dispatched.load(Ordering::SeqCst) {
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
    poller: Arc<dyn Poller>,
    notification: Arc<DeadlineWake>,
    waits: Receiver<Wait>,
) {
    while let Ok(wait) = waits.recv() {
        // Wait for readiness WITHOUT holding the source mutex, so the owner
        // (render, teardown, capability application) can lock the source
        // freely while this thread is parked. The poller is level-triggered
        // and `poll` takes `&self`, so a concurrent readiness check here and a
        // later drain by the task both observe the same readiness. One wait
        // per dispatched request; the task re-arms for the next.
        if !wait.shutdown.load(Ordering::SeqCst) {
            // Briefly lock only to read the nearest ESC/paste decode deadline.
            // The guard is dropped at the end of this `map`, before the
            // blocking wait below, so the source stays lockable while parked.
            // Without honoring the deadline, a buffered partial escape with no
            // further input would never wake the task and a lone Esc would
            // hang. On a poisoned lock, skip the wait and let the task surface
            // the poison on its next poll.
            if let Ok(timeout) = source.lock().map(|src| src.effective_timeout(None)) {
                #[cfg(test)]
                if let Some(started) = &wait.started {
                    let _ = started.try_send(timeout);
                }
                let mut ready = [false; READY_SLOTS + 1];
                // Any return (readiness, elapsed deadline, wake, or error)
                // hands back to the task, which drains and decodes under its
                // own `try_lock`; stray wakes are harmless.
                match poller.poll(&mut ready, timeout) {
                    Ok(_) if ready[READY_SLOTS] => {
                        if let Err(error) = notification.waker.clear() {
                            notification.fail(error);
                        }
                    }
                    Err(error) => notification.fail(error),
                    Ok(_) => {}
                }
            }
        }
        wait.dispatched.store(false, Ordering::SeqCst);
        notification.wake_task();
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

    #[test]
    fn source_lockable_while_stream_parked() {
        // Path 2: the waiter must not hold the source lock while parked, so
        // the owner (render, teardown, capability application) can lock the
        // source even with no input pending. Under the old lock-while-parked
        // design this would block until input arrived.
        let (rx, tx) = make_pipe();
        let shared = Arc::new(Mutex::new(EventSource::new(rx).unwrap()));
        let mut stream = EventStream::from_shared(Arc::clone(&shared));

        // Poll once with no input: the stream parks and the waiter thread
        // enters its blocking, lock-free readiness wait.
        let flag = Arc::new(FlagWaker(AtomicBool::new(false)));
        let waker = Waker::from(Arc::clone(&flag));
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(
            Pin::new(&mut stream).poll_next(&mut cx),
            Poll::Pending
        ));
        // Give the waiter time to pass its brief deadline peek and reach the
        // blocking poll.
        std::thread::sleep(Duration::from_millis(20));

        assert!(
            shared.try_lock().is_ok(),
            "source lock is held by the parked waiter (deadlock risk)"
        );

        // The stream still delivers input afterwards.
        write_bytes(&tx, b"q");
        let q = next_blocking(&mut stream).unwrap().unwrap();
        assert!(matches!(q, Event::KeyPress(k) if k.code == KeyCode::Char('q')));
    }

    #[test]
    fn changing_a_parked_string_deadline_wakes_the_stream() {
        struct Notify(SyncSender<()>);
        impl Wake for Notify {
            fn wake(self: Arc<Self>) {
                let _ = self.0.try_send(());
            }
        }

        for timeout in [None, Some(Duration::from_secs(60))] {
            let (rx, _tx) = make_pipe();
            let mut source = EventSource::new(rx)
                .unwrap()
                .with_string_idle_timeout(timeout);
            source.feed_bytes(b"\x1b]qqqq");
            let shared = Arc::new(Mutex::new(source));
            let mut stream = EventStream::from_shared(Arc::clone(&shared));
            let (started_tx, started_rx) = mpsc::sync_channel(1);
            stream.wait_started = Some(started_tx);
            let (wake_tx, wake_rx) = mpsc::sync_channel(1);
            let waker = Waker::from(Arc::new(Notify(wake_tx)));
            let mut cx = Context::from_waker(&waker);
            assert!(Pin::new(&mut stream).poll_next(&mut cx).is_pending());
            let captured = started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            match timeout {
                None => assert_eq!(captured, None),
                Some(_) => assert!(captured.unwrap() > Duration::from_secs(30)),
            }

            shared
                .lock()
                .unwrap()
                .set_string_idle_timeout(Some(Duration::ZERO));
            wake_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("deadline change must wake the parked stream without more input");
            match Pin::new(&mut stream).poll_next(&mut cx) {
                Poll::Ready(Some(Ok(event))) => {
                    assert_eq!(event, Event::Unknown(b"\x1b]qqqq".to_vec()))
                }
                other => panic!("expected expired string, got {other:?}"),
            }
        }
    }

    #[test]
    fn deadline_notifications_leave_sync_wake_behavior_intact() {
        let (rx, tx) = make_pipe();
        let mut source = EventSource::new(rx).unwrap().with_string_idle_timeout(None);
        source.feed_bytes(b"\x1b]qqqq");
        let shared = Arc::new(Mutex::new(source));
        let mut stream = EventStream::from_shared(Arc::clone(&shared));
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        stream.wait_started = Some(started_tx);
        let waker = Waker::from(Arc::new(FlagWaker(AtomicBool::new(false))));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut stream).poll_next(&mut cx).is_pending());
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            None
        );

        {
            let mut source = shared.lock().unwrap();
            source.set_string_idle_timeout(Some(Duration::ZERO));
            assert_eq!(
                source.read().unwrap(),
                Event::Unknown(b"\x1b]qqqq".to_vec())
            );
            source.waker().wake().unwrap();
            assert_eq!(
                source.read().unwrap_err().kind(),
                io::ErrorKind::Interrupted
            );
        }

        drop(stream);
        write_bytes(&tx, b"x");
        assert!(matches!(shared.lock().unwrap().read().unwrap(),
            Event::KeyPress(k) if k.code == KeyCode::Char('x')));
    }

    #[test]
    fn lone_esc_resolves_through_the_stream() {
        // A bare Esc has no follow-up bytes, so it only resolves once its
        // decode deadline elapses. The stream must honor that deadline: the
        // waiter wakes on the elapsed timeout and the next poll drains the
        // resolved key. Guards the `dispatched`-gated drain path.
        let (rx, tx) = make_pipe();
        let mut stream = EventSource::new(rx).unwrap().into_stream();
        write_bytes(&tx, b"\x1b");
        let esc = next_blocking(&mut stream).unwrap().unwrap();
        assert!(matches!(esc, Event::KeyPress(k) if k.code == KeyCode::Escape));
    }

    fn _assert_send_sync<T: Send + Sync>() {}
    #[test]
    fn stream_is_send_sync() {
        _assert_send_sync::<EventStream<File>>();
    }
}

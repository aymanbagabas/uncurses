//! Unix-specific construction and event-pump for [`EventSource`].
//!
//! Holds the platform read path: a self-pipe waker, a SIGWINCH self-pipe
//! that surfaces [`Event::Resize`], and the byte-oriented `Read` +
//! [`Decoder`] decode loop. The cross-platform surface (the struct, the
//! shared methods) lives in [`super::source`].

#![cfg(unix)]

use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;

use super::decode::{Decoder, DecoderFlags};
use super::pending::Pending;
use super::poll::PollFd;
use super::sigwinch as winch;
use super::source::{
    DEFAULT_BUFFER_CAPACITY, DEFAULT_ESC_TIMEOUT, DEFAULT_PASTE_IDLE_TIMEOUT, EventSource, Input,
    Waker,
};
use crate::event::Event;
use crate::terminal::size::get_window_size;

pub(super) struct UnixWakerInner {
    /// Write end of the self-pipe. Non-blocking; closed on drop.
    tx: OwnedFd,
}

impl UnixWakerInner {
    pub(super) fn wake(&self) -> io::Result<()> {
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

// ---------------------------------------------------------------------------
// Unix implementation
// ---------------------------------------------------------------------------

impl<I> EventSource<I>
where
    I: Input,
{
    /// Build a new event source for `input`. The handle is also used as
    /// the `TIOCGWINSZ` target whenever SIGWINCH fires, so it must refer
    /// to the terminal whose size the caller cares about (typically the
    /// controlling tty — any fd that names it works).
    ///
    /// Timeouts start at their documented defaults; override them with
    /// [`EventSource::with_esc_timeout`] and
    /// [`EventSource::with_paste_idle_timeout`].
    pub fn new(input: I) -> io::Result<Self> {
        let (pipe_rx, pipe_tx) = make_self_pipe()?;
        let (winch_rx, winch_tx) = make_self_pipe()?;
        let winch_sub = winch::subscribe(winch_tx.as_raw_fd())?;
        let waker = Waker::from_unix_inner(Arc::new(UnixWakerInner { tx: pipe_tx }));

        // Watch input, wake pipe, and winch pipe — in the fixed index
        // order the pump/ingest path relies on. Detect a tty input fd up
        // front so Darwin can pick the select backend (its kqueue spins
        // on tty character devices).
        let input_fd = input.as_fd().as_raw_fd();
        let input_is_tty = unsafe { libc::isatty(input_fd) } == 1;
        let fds: [PollFd; 3] = [input_fd, pipe_rx.as_raw_fd(), winch_rx.as_raw_fd()];
        let poller = super::poll::new_poller(&fds, input_is_tty)?;

        Ok(Self {
            input,
            parser: Decoder::new(DecoderFlags::empty()),
            pending: Pending::with_capacity(DEFAULT_BUFFER_CAPACITY),
            esc_timeout: DEFAULT_ESC_TIMEOUT,
            esc_deadline: None,
            paste_idle_timeout: Some(DEFAULT_PASTE_IDLE_TIMEOUT),
            paste_deadline: None,
            queue: VecDeque::with_capacity(16),
            waker,
            handle_resize: true,
            poller,
            pipe_rx,
            winch_rx,
            _winch_tx: winch_tx,
            _winch_sub: winch_sub,
            last_size: None,
        })
    }

    /// Drain the wake pipe after a [`Waker`] fired. Platform hook for
    /// [`EventSource::fill`].
    pub(super) fn drain_wake(&mut self) {
        drain_pipe(self.pipe_rx.as_raw_fd());
    }

    /// Read whatever input bytes are ready and run them through the
    /// decoder. Platform hook for [`EventSource::fill`].
    pub(super) fn drain_input(&mut self) -> io::Result<()> {
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

    pub(super) fn handle_winch(&mut self) {
        drain_pipe(self.winch_rx.as_raw_fd());
        // When in-band resize reporting is enabled the host disables
        // this path; the terminal delivers resizes through the decoder
        // instead, so emitting here too would duplicate every event.
        if !self.handle_resize {
            return;
        }
        let new_size = match get_window_size(self.input.as_fd()) {
            Ok(sz) => sz,
            Err(_) => return,
        };
        if Some(new_size) == self.last_size {
            return;
        }
        self.last_size = Some(new_size);
        self.emit(Event::Resize(new_size));
    }
}

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

fn drain_pipe(fd: i32) {
    let mut buf = [0u8; 32];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n <= 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::KeyCode;
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::thread;
    use std::time::Duration;
    use std::time::Instant;

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
        EventSource::new(input)
            .unwrap()
            .with_esc_timeout(Duration::from_millis(50))
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
    fn unread_returns_event_to_front_of_queue() {
        let (rx, tx) = make_pipe();
        let mut src = new_reader(rx);
        write_bytes(&tx, b"ab");
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        // Pull both events, then put the first one back: a later read must
        // see it again before the rest of the queue.
        let a = src.try_read().expect("a queued");
        let b = src.try_read().expect("b queued");
        assert!(matches!(a, Event::KeyPress(ref k) if k.code == KeyCode::Char('a')));
        assert!(matches!(b, Event::KeyPress(ref k) if k.code == KeyCode::Char('b')));
        src.unread(b);
        src.unread(a);
        assert!(matches!(src.read().unwrap(), Event::KeyPress(k) if k.code == KeyCode::Char('a')));
        assert!(matches!(src.read().unwrap(), Event::KeyPress(k) if k.code == KeyCode::Char('b')));
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
    fn esc_resolves_before_late_continuation_byte() {
        // A buffered partial ESC whose deadline elapses must resolve to a
        // bare Esc before a continuation byte that arrives afterward is
        // read, so the two never merge into an Alt-modified key.
        let (rx, tx) = make_pipe();
        let mut src = new_reader(rx); // 50ms esc timeout
        write_bytes(&tx, b"\x1b");
        // Drain the lone ESC so its disambiguation deadline is armed; the
        // queue stays empty because the sequence is still partial.
        assert!(!src.poll(Some(Duration::from_millis(0))).unwrap());
        // Let the deadline elapse without draining, then deliver a byte
        // that would otherwise complete an ESC-prefixed sequence.
        thread::sleep(Duration::from_millis(80));
        write_bytes(&tx, b"a");
        let first = src.read().unwrap();
        assert!(
            matches!(&first, Event::KeyPress(k) if k.code == KeyCode::Escape),
            "expected bare Esc, got {:?}",
            first
        );
        let second = src.read().unwrap();
        assert!(
            matches!(&second, Event::KeyPress(k) if k.code == KeyCode::Char('a')),
            "expected 'a', got {:?}",
            second
        );
    }

    #[test]
    fn paste_idle_timeout_synthesizes_paste_end() {
        let (rx, tx) = make_pipe();
        let mut src = EventSource::new(rx)
            .unwrap()
            .with_paste_idle_timeout(Some(Duration::from_millis(40)));
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
        let mut src = EventSource::new(rx)
            .unwrap()
            .with_paste_idle_timeout(Some(Duration::from_millis(500)));
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
        let mut src = EventSource::new(rx).unwrap().with_paste_idle_timeout(None);
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
        let mut src = EventSource::new(rx).unwrap().with_paste_idle_timeout(None);
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
        let mut src = EventSource::new(rx)
            .unwrap()
            .with_esc_timeout(Duration::from_millis(20))
            .with_paste_idle_timeout(Some(Duration::from_secs(5)));
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
        let mut src = EventSource::new(rx)
            .unwrap()
            .with_esc_timeout(Duration::from_millis(20));
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
    fn handle_resize_false_suppresses_sigwinch_resize_event() {
        // With resize handling disabled (the host has enabled in-band
        // reports), a SIGWINCH must drain its wake pipe but surface no
        // Event::Resize — the decoder delivers resizes in-band instead.
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
        assert!(src.handle_resize());
        src.set_handle_resize(false);
        assert!(!src.handle_resize());
        src.last_size = None;
        unsafe { libc::raise(libc::SIGWINCH) };
        // No event is produced; the poll runs to its (short) timeout.
        assert!(!src.poll(Some(Duration::from_millis(50))).unwrap());
        assert!(src.try_read().is_none());
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

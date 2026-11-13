//! Unix-specific construction and readiness servicing for [`EventSource`].
//!
//! ## Purpose
//!
//! This module supplies the Unix [`EventSource::new`] implementation and the
//! platform hooks used by the shared source pump: drain the wake self-pipe, read
//! ready bytes, and surface `SIGWINCH` as [`Event::Resize`].
//!
//! ```text
//! [input fd] ─┐
//! [wake pipe] ├─▶ Poller ──▶ EventSource::fill
//! [winch pipe]┘        ├─ wake: Interrupted
//!                      ├─ input: read + decode
//!                      └─ winch: query Winsize + Resize
//! ```
//!
//! ## Key types
//!
//! `UnixWakerInner` owns *both* ends of a non-blocking self-pipe. The shared
//! [`Waker`] wraps it so other threads can interrupt blocking reads without
//! touching decoder state. Both ends live behind the same `Arc` because a
//! `Waker` may outlive its source, and writing to a pipe whose reader has
//! closed raises a synchronous `SIGPIPE`.
//!
//! ## Gotchas
//!
//! The input fd is also used for `TIOCGWINSZ`, so it should refer to the same
//! terminal whose size the caller wants. When in-band resize reports are
//! enabled, [`EventSource::set_handle_resize`] should disable the SIGWINCH path
//! to avoid duplicate resize events.
#![cfg(unix)]

use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(feature = "async")]
use std::sync::Arc;

use super::decode::{Decoder, DecoderFlags};
use super::pending::Pending;
use super::poll::PollFd;
use super::sigwinch as winch;
use super::source::{
    DEFAULT_BUFFER_CAPACITY, DEFAULT_ESC_TIMEOUT, DEFAULT_PASTE_IDLE_TIMEOUT,
    DEFAULT_STRING_IDLE_TIMEOUT, EventSource, Input, SourceFlags, Waker,
};
use crate::event::Event;
use crate::terminal::get_window_size;

pub(super) struct UnixWakerInner {
    /// Write end of the self-pipe. Non-blocking; closed on drop.
    tx: OwnedFd,
    /// Read end, kept here rather than in [`EventSource`] so that it cannot be
    /// closed while a `Waker` clone still holds `tx`. A write to a pipe whose
    /// reader is gone raises a *synchronous* `SIGPIPE` on the calling thread,
    /// which no error handling can intercept — invisible in a Rust binary
    /// (std sets `SIG_IGN` at startup) but fatal inside a C or Go host.
    /// `Waker` is public, `Clone` and `Send`, so it legitimately outlives the
    /// source it came from; sharing both ends behind the same `Arc` makes that
    /// safe instead of merely unlikely.
    rx: OwnedFd,
}

impl UnixWakerInner {
    pub(super) fn new() -> io::Result<Self> {
        let (rx, tx) = make_self_pipe()?;
        Ok(Self { tx, rx })
    }

    #[cfg(feature = "async")]
    pub(super) fn clear(&self) -> io::Result<()> {
        drain_pipe(self.read_fd());
        Ok(())
    }

    pub(super) fn read_fd(&self) -> std::os::fd::RawFd {
        self.rx.as_raw_fd()
    }

    pub(super) fn wake(&self) -> io::Result<()> {
        let buf = b"w";
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
    /// Build a new Unix event source for `input`.
    ///
    /// The handle is used both for byte reads and as the `TIOCGWINSZ` target
    /// when `SIGWINCH` fires, so it should refer to the terminal whose size the
    /// caller cares about. Construction creates two non-blocking self-pipes,
    /// subscribes to the shared SIGWINCH fan-out, and registers the input, wake,
    /// and resize fds with the selected poll backend.
    ///
    /// Timeouts start at [`DEFAULT_ESC_TIMEOUT`], [`DEFAULT_PASTE_IDLE_TIMEOUT`],
    /// and [`DEFAULT_STRING_IDLE_TIMEOUT`]. Override them with
    /// [`EventSource::with_esc_timeout`],
    /// [`EventSource::with_paste_idle_timeout`], and
    /// [`EventSource::with_string_idle_timeout`].
    ///
    /// Resize deduplication starts from the size `input` reports here, so a
    /// `SIGWINCH` that does not actually change the size produces no
    /// [`Event::Resize`].
    ///
    /// `flags` selects the input source behavior.
    ///
    /// Returns any OS error from pipe creation, fd configuration, SIGWINCH
    /// subscription, or poller construction. It does not read from `input`.
    pub fn new_with_flags(input: I, flags: SourceFlags) -> io::Result<Self> {
        let winch_sub = winch::subscribe()?;
        let waker = Waker::new()?;

        // Watch input, wake pipe, and winch pipe — in the fixed index
        // order the pump/ingest path relies on. Detect a tty input fd up
        // front so Darwin can pick the select backend (its kqueue spins
        // on tty character devices).
        let input_fd = input.as_fd().as_raw_fd();
        let input_is_tty = unsafe { libc::isatty(input_fd) } == 1;
        // Seed the resize dedupe with the size we start at, so the first wake
        // reports a resize only if one actually happened. Without this any
        // first wake emits, including one caused by a late handler write that
        // landed in this slot's pooled pipe just after it was leased to us.
        // `None` on a non-tty, where there is no size to compare against.
        let last_size = get_window_size(input.as_fd()).ok();
        let fds: [PollFd; 3] = [input_fd, waker.poll_fd(), winch_sub.read_fd()];
        let poller = super::poll::new_poller(&fds, input_is_tty)?;

        Ok(Self {
            input,
            flags,
            parser: Decoder::new(DecoderFlags::empty()),
            pending: Pending::with_capacity(DEFAULT_BUFFER_CAPACITY),
            esc_timeout: DEFAULT_ESC_TIMEOUT,
            esc_deadline: None,
            paste_idle_timeout: Some(DEFAULT_PASTE_IDLE_TIMEOUT),
            paste_deadline: None,
            string_idle_timeout: Some(DEFAULT_STRING_IDLE_TIMEOUT),
            string_deadline: None,
            #[cfg(feature = "async")]
            deadline_wakers: Vec::new(),
            queue: VecDeque::with_capacity(16),
            pending_error: None,
            waker,
            handle_resize: true,
            poller,
            winch_sub,
            last_size,
        })
    }

    #[cfg(feature = "async")]
    pub(super) fn waiter_poller(
        &self,
        notification: &Waker,
    ) -> io::Result<Arc<dyn super::poll::Poller>> {
        let input_fd = self.input.as_fd().as_raw_fd();
        let fds = [
            input_fd,
            self.waker.poll_fd(),
            self.winch_sub.read_fd(),
            notification.poll_fd(),
        ];
        super::poll::new_poller(&fds, unsafe { libc::isatty(input_fd) } == 1)
    }

    /// Drain pending wake bytes after a [`Waker`] fired.
    ///
    /// Platform hook for [`EventSource::fill`]. Multiple wake bytes may have
    /// coalesced; draining them all lets a subsequent poll block again.
    pub(super) fn drain_wake(&mut self) {
        drain_pipe(self.waker.pipe_read_fd());
    }

    /// Read ready input bytes and run them through the decoder.
    ///
    /// Platform hook for [`EventSource::fill`]. `Interrupted` and `WouldBlock`
    /// reads are treated as a transient absence of bytes. A zero-length read is
    /// surfaced as [`io::ErrorKind::UnexpectedEof`]. If the pending buffer is
    /// already full, it is cleared because its capacity is the hard cap on one
    /// undecoded sequence.
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
        drain_pipe(self.winch_sub.read_fd());
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

pub(super) fn make_self_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
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

pub(super) fn drain_pipe(fd: i32) {
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
    fn source_flags_are_separate_from_decoder_flags() {
        let (rx, _tx) = make_pipe();
        assert_eq!(EventSource::new(rx).unwrap().flags(), SourceFlags::empty());
        let (rx, _tx) = make_pipe();
        let mut src = EventSource::new_with_flags(rx, SourceFlags::VT_INPUT).unwrap();
        assert_eq!(src.flags(), SourceFlags::VT_INPUT);
        src.set_decoder_flags(DecoderFlags::LF_IS_ENTER);
        src.set_flags(SourceFlags::empty());
        assert_eq!(src.flags(), SourceFlags::empty());
        assert_eq!(src.decoder_flags(), DecoderFlags::LF_IS_ENTER);
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
    fn cursor_reports_and_f3_preserve_event_order() {
        use crate::event::{Key, KeyModifiers};
        use crate::layout::Position;

        let cpr = |x, y, page| Event::CursorPosition {
            position: Position::new(x, y),
            page,
        };
        let f3 = |modifiers| Event::KeyPress(Key::new(KeyCode::F(3), modifiers));
        for prefix in [b"\x1b[".as_slice(), b"\x9b".as_slice()] {
            for (body, expected) in [
                ("R", Some(vec![f3(KeyModifiers::empty())])),
                ("1R", Some(vec![f3(KeyModifiers::empty())])),
                ("1;5R", Some(vec![f3(KeyModifiers::CTRL), cpr(4, 0, 0)])),
                ("5;10R", Some(vec![cpr(9, 4, 0)])),
                ("1;17R", Some(vec![cpr(16, 0, 0)])),
                ("?1;5R", Some(vec![cpr(4, 0, 0)])),
                ("?5;10;2R", Some(vec![cpr(9, 4, 2)])),
                ("?5;10;R", Some(vec![cpr(9, 4, 0)])),
                ("?1;5;2;3R", Some(vec![cpr(4, 0, 2)])),
                ("1;2;3R", None),
                ("?R", None),
                (">1;5R", None),
                ("?1;5 R", None),
            ] {
                let mut wire = [prefix, body.as_bytes()].concat();
                let mut expected =
                    expected.unwrap_or_else(|| vec![Event::UnknownCsi(wire.clone())]);
                wire.push(b'x');
                expected.push(Event::KeyPress(Key::new(
                    KeyCode::Char('x'),
                    KeyModifiers::empty(),
                )));
                let (rx, tx) = make_pipe();
                let mut src = new_reader(rx);
                write_bytes(&tx, &wire);
                for event in expected {
                    assert!(src.poll(Some(Duration::from_secs(1))).unwrap(), "{wire:?}");
                    assert_eq!(src.read().unwrap(), event, "{wire:?}");
                }
                assert!(src.try_read().is_none(), "{wire:?}");
            }
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
    fn ambiguous_introducers_expire_as_alt_keys() {
        use crate::event::{Key, KeyModifiers};

        for flags in [DecoderFlags::empty(), DecoderFlags::CTRL_OPEN_BRACKET] {
            for byte in b"[O]P_X^" {
                let (rx, tx) = make_pipe();
                let mut src = new_reader(rx)
                    .with_esc_timeout(Duration::ZERO)
                    .with_decoder_flags(flags);
                write_bytes(&tx, &[0x1b, *byte]);
                src.fill(Some(Duration::ZERO)).unwrap();
                assert!(src.try_read().is_none());
                src.fill(Some(Duration::ZERO)).unwrap();
                assert_eq!(
                    src.try_read(),
                    Some(Event::KeyPress(
                        Key::new(KeyCode::Char(*byte as char), KeyModifiers::ALT).normalized()
                    )),
                );
                assert!(src.try_read().is_none());
            }
        }
    }

    #[test]
    fn expired_input_is_delivered_before_eof() {
        use crate::event::{Key, KeyModifiers};

        for (input, expected) in [
            (
                b"\x1b[".as_slice(),
                Event::KeyPress(Key::new(KeyCode::Char('['), KeyModifiers::ALT)),
            ),
            (b"\x1b]q", Event::Unknown(b"\x1b]q".to_vec())),
        ] {
            for expired in [false, true] {
                let (rx, tx) = make_pipe();
                let timeout = if expired {
                    Duration::ZERO
                } else {
                    Duration::from_secs(60)
                };
                let mut src = new_reader(rx)
                    .with_esc_timeout(timeout)
                    .with_string_idle_timeout(Some(timeout));
                write_bytes(&tx, input);
                assert!(!src.poll(Some(Duration::ZERO)).unwrap());
                drop(tx);

                if expired {
                    assert_eq!(src.read().unwrap(), expected);
                }
                assert_eq!(src.read().unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
            }
        }
    }

    #[test]
    fn expired_input_is_delivered_before_other_read_errors() {
        use crate::event::{Key, KeyModifiers};

        struct ErrorAfterRead {
            input: File,
            fail: bool,
        }

        impl std::os::fd::AsFd for ErrorAfterRead {
            fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
                self.input.as_fd()
            }
        }

        impl std::io::Read for ErrorAfterRead {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.fail {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                self.fail = true;
                self.input.read(buf)
            }
        }

        for (input, expected) in [
            (
                b"\x1b[".as_slice(),
                Event::KeyPress(Key::new(KeyCode::Char('['), KeyModifiers::ALT)),
            ),
            (b"\x1b]q", Event::Unknown(b"\x1b]q".to_vec())),
        ] {
            let (rx, tx) = make_pipe();
            let mut src = EventSource::new(ErrorAfterRead {
                input: rx,
                fail: false,
            })
            .unwrap()
            .with_esc_timeout(Duration::ZERO)
            .with_string_idle_timeout(Some(Duration::ZERO));
            write_bytes(&tx, input);
            assert!(!src.poll(Some(Duration::ZERO)).unwrap());
            write_byte(&tx, b'x');

            assert_eq!(src.read().unwrap(), expected);
            assert_eq!(src.read().unwrap_err().raw_os_error(), Some(libc::EIO));
        }
    }

    #[test]
    fn ready_continuations_refresh_sequence_deadlines() {
        use crate::event::{Key, KeyModifiers};

        for prefix in [b"\x1b[".as_slice(), b"\x9b"] {
            for overdue in [false, true] {
                let (rx, tx) = make_pipe();
                let mut src = new_reader(rx);
                write_bytes(&tx, prefix);
                src.fill(Some(Duration::ZERO)).unwrap();
                for fragment in [b"1".as_slice(), b";", b"2"] {
                    let old = if overdue {
                        Instant::now() - Duration::from_secs(1)
                    } else {
                        Instant::now() + Duration::from_secs(1)
                    };
                    src.esc_deadline = Some(old);
                    write_bytes(&tx, fragment);
                    src.fill(Some(Duration::ZERO)).unwrap();
                    assert!(src.try_read().is_none(), "{fragment:?}");
                    let new = src.esc_deadline.unwrap();
                    assert!(if overdue { new > old } else { new < old });
                }
                write_bytes(&tx, b"A");
                src.fill(Some(Duration::ZERO)).unwrap();
                assert_eq!(
                    src.try_read(),
                    Some(Event::KeyPress(Key::new(KeyCode::Up, KeyModifiers::SHIFT))),
                );
                assert!(src.try_read().is_none());
            }
        }
    }

    #[test]
    fn control_string_bodies_wait_across_reads_and_timeouts() {
        for (introducer, c1, expected) in [
            (b']', 0x9d, Event::UnknownOsc(b"qqqq".to_vec())),
            (b'P', 0x90, Event::UnknownDcs(b"qqqq".to_vec())),
            (b'_', 0x9f, Event::UnknownApc(b"qqqq".to_vec())),
            (b'X', 0x98, Event::UnknownSos(b"qqqq".to_vec())),
            (b'^', 0x9e, Event::UnknownPm(b"qqqq".to_vec())),
        ] {
            for prefix in [vec![0x1b, introducer], vec![c1]] {
                for ending in [b"\x1b\\".as_slice(), b"\x9c"] {
                    let (rx, tx) = make_pipe();
                    let mut src = new_reader(rx).with_esc_timeout(Duration::ZERO);
                    write_bytes(&tx, &[prefix.as_slice(), b"q"].concat());
                    src.fill(Some(Duration::ZERO)).unwrap();
                    src.fill(Some(Duration::ZERO)).unwrap();
                    assert!(src.try_read().is_none(), "{prefix:?}");
                    for fragment in [b"qqq".as_slice(), &ending[..ending.len() - 1]] {
                        if !fragment.is_empty() {
                            write_bytes(&tx, fragment);
                        }
                        src.fill(Some(Duration::ZERO)).unwrap();
                        src.fill(Some(Duration::ZERO)).unwrap();
                        assert!(src.try_read().is_none(), "{prefix:?}");
                    }
                    write_bytes(&tx, &ending[ending.len() - 1..]);
                    assert_eq!(src.read().unwrap(), expected);
                    write_bytes(&tx, b"\x1b");
                    assert_eq!(
                        src.read().unwrap(),
                        Event::KeyPress(crate::event::Key::new(
                            KeyCode::Escape,
                            crate::event::KeyModifiers::empty()
                        )),
                    );
                    assert!(src.try_read().is_none());
                }
            }
        }
    }

    #[test]
    fn osc_payloads_wait_for_terminator_after_escape_timeout() {
        for prefix in [b"\x1b]".as_slice(), b"\x9d"] {
            for ending in [b"\x1b\\".as_slice(), b"\x07", b"\x9c"] {
                for (body, expected) in [
                    (
                        b"52;c;qqqq".as_slice(),
                        Event::Clipboard {
                            selection: crate::event::ClipboardSelection::System,
                            content: "qqqq".into(),
                        },
                    ),
                    (
                        b"72;t=r:x=1;qqqq",
                        Event::DragDrop {
                            options: vec![("t".into(), "r".into()), ("x".into(), "1".into())],
                            payload: b"qqqq".to_vec(),
                        },
                    ),
                    (b"999;qqqq", Event::UnknownOsc(b"999;qqqq".to_vec())),
                ] {
                    let (rx, tx) = make_pipe();
                    let mut src = new_reader(rx).with_esc_timeout(Duration::ZERO);
                    write_bytes(&tx, &[prefix, body].concat());
                    src.fill(Some(Duration::ZERO)).unwrap();
                    src.fill(Some(Duration::ZERO)).unwrap();
                    let events: Vec<_> = std::iter::from_fn(|| src.try_read()).collect();
                    assert!(
                        events.is_empty(),
                        "partial OSC payload escaped as input: {events:?}"
                    );
                    write_bytes(&tx, ending);
                    assert_eq!(src.read().unwrap(), expected);
                    assert!(src.try_read().is_none());
                }
            }
        }
    }

    #[test]
    fn escape_sequence_interrupts_a_string_without_an_idle_timeout() {
        use crate::event::{Key, KeyModifiers};

        let (rx, tx) = make_pipe();
        let mut src = new_reader(rx)
            .with_esc_timeout(Duration::ZERO)
            .with_string_idle_timeout(None);
        write_bytes(&tx, b"\x1b]qqqq\x1b");
        src.fill(Some(Duration::ZERO)).unwrap();
        src.fill(Some(Duration::ZERO)).unwrap();
        assert!(src.try_read().is_none());
        write_bytes(&tx, b"[A");
        src.fill(Some(Duration::ZERO)).unwrap();
        assert_eq!(src.try_read(), Some(Event::Unknown(b"\x1b]qqqq".to_vec())));
        assert_eq!(
            src.try_read(),
            Some(Event::KeyPress(Key::new(
                KeyCode::Up,
                KeyModifiers::empty()
            ))),
        );
        assert!(src.try_read().is_none());
        assert_eq!(src.effective_timeout(None), None);
    }

    #[test]
    fn string_idle_timeout_reports_whole_sequence_and_recovers() {
        use crate::event::{Key, KeyModifiers};

        for (introducer, c1) in [
            (b']', 0x9d),
            (b'P', 0x90),
            (b'_', 0x9f),
            (b'X', 0x98),
            (b'^', 0x9e),
        ] {
            for prefix in [vec![0x1b, introducer], vec![c1]] {
                let (rx, tx) = make_pipe();
                let mut src = new_reader(rx)
                    .with_string_idle_timeout(Some(Duration::ZERO))
                    .with_paste_idle_timeout(None);
                let wire = [prefix.as_slice(), b"qqqq"].concat();
                write_bytes(&tx, &wire);
                src.fill(Some(Duration::ZERO)).unwrap();
                assert!(src.try_read().is_none());
                assert_eq!(src.effective_timeout(None), Some(Duration::ZERO));
                assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
                assert_eq!(src.try_read(), Some(Event::Unknown(wire)));
                assert!(src.try_read().is_none());
                assert_eq!(src.effective_timeout(None), None);
                write_byte(&tx, b'q');
                assert_eq!(
                    src.read().unwrap(),
                    Event::KeyPress(Key::new(KeyCode::Char('q'), KeyModifiers::empty())),
                );
            }
        }
    }

    #[test]
    fn ready_string_data_refreshes_idle_deadline() {
        for overdue in [false, true] {
            let (rx, tx) = make_pipe();
            let mut src = new_reader(rx);
            write_bytes(&tx, b"\x1b]q");
            src.fill(Some(Duration::ZERO)).unwrap();
            for _ in 0..3 {
                let old = if overdue {
                    Instant::now() - Duration::from_secs(1)
                } else {
                    Instant::now() + Duration::from_secs(60)
                };
                src.string_deadline = Some(old);
                write_byte(&tx, b'q');
                src.fill(Some(Duration::ZERO)).unwrap();
                assert!(src.try_read().is_none());
                let new = src.string_deadline.unwrap();
                assert!(if overdue { new > old } else { new < old });
            }
            write_bytes(&tx, b"\x1b\\");
            assert_eq!(src.read().unwrap(), Event::UnknownOsc(b"qqqq".to_vec()));
            assert_eq!(src.effective_timeout(None), None);
        }
    }

    #[test]
    fn string_idle_timeout_can_be_disabled_during_a_string() {
        let (rx, tx) = make_pipe();
        let mut src = new_reader(rx).with_string_idle_timeout(Some(Duration::ZERO));
        write_bytes(&tx, b"\x1b]q");
        src.fill(Some(Duration::ZERO)).unwrap();
        src.set_string_idle_timeout(None);
        assert_eq!(src.effective_timeout(None), None);
        assert!(!src.poll(Some(Duration::ZERO)).unwrap());
        write_bytes(&tx, b"\x1b\\");
        assert_eq!(src.read().unwrap(), Event::UnknownOsc(b"q".to_vec()));
    }

    #[test]
    fn paste_idle_timeout_synthesizes_paste_end() {
        let (rx, tx) = make_pipe();
        let mut src = EventSource::new(rx)
            .unwrap()
            .with_string_idle_timeout(Some(Duration::ZERO))
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

    /// The flags reach the decoder, and what they change is which key a byte
    /// is called. `0x0d` is `enter` by default and `ctrl+m` when a caller
    /// asks for that reading.
    #[test]
    fn the_decoder_flags_a_caller_chose_are_the_ones_used() {
        use crate::event::DecoderFlags;

        let (rx, tx) = make_pipe();
        let mut src = EventSource::new(rx).unwrap();
        write_byte(&tx, 0x0d);
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        let Event::KeyPress(k) = src.read().unwrap() else {
            panic!("expected a key")
        };
        assert_eq!(k.code, KeyCode::Enter, "the reading a terminal means");

        let (rx, tx) = make_pipe();
        let mut src = EventSource::new(rx)
            .unwrap()
            .with_decoder_flags(DecoderFlags::CTRL_M);
        assert_eq!(src.decoder_flags(), DecoderFlags::CTRL_M);
        write_byte(&tx, 0x0d);
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        let Event::KeyPress(k) = src.read().unwrap() else {
            panic!("expected a key")
        };
        assert_eq!(k.code, KeyCode::Char('m'));
        assert_eq!(k.modifiers, crate::event::KeyModifiers::CTRL);
    }

    /// `0x1b` is the Escape key and `ctrl+[` both, and an application that
    /// asked for the Ctrl reading gets it for a real lone `ESC`: the one the
    /// source resolves itself once the escape deadline passes, which is the
    /// only way that byte ever arrives alone.
    #[test]
    fn a_lone_escape_from_the_terminal_follows_the_flag() {
        use crate::event::DecoderFlags;

        let (rx, tx) = make_pipe();
        let mut src = EventSource::new(rx)
            .unwrap()
            .with_esc_timeout(Duration::from_millis(20))
            .with_decoder_flags(DecoderFlags::CTRL_OPEN_BRACKET);
        write_byte(&tx, 0x1b);
        // The first poll buffers it; the deadline is what resolves it, so
        // give the source long enough to reach its own.
        let _ = src.poll(Some(Duration::from_millis(50))).unwrap();
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        let Event::KeyPress(k) = src.read().unwrap() else {
            panic!("expected a key")
        };
        assert_eq!(k.code, KeyCode::Char('['));
        assert_eq!(k.modifiers, crate::event::KeyModifiers::CTRL);
    }

    /// The same choice on a source already in use, which is the seam
    /// [`Program::init_with`](crate::program::Program::init_with) reaches
    /// through when it carries `ProgramOptions::legacy_keys`.
    #[test]
    fn the_flags_can_be_chosen_on_a_source_already_built() {
        use crate::event::DecoderFlags;

        let (rx, tx) = make_pipe();
        let mut src = EventSource::new(rx).unwrap();
        assert!(
            src.decoder_flags().is_empty(),
            "a source reads each as the named key until told otherwise"
        );
        src.set_decoder_flags(DecoderFlags::CTRL_I);
        write_byte(&tx, 0x09);
        assert!(src.poll(Some(Duration::from_secs(1))).unwrap());
        let Event::KeyPress(k) = src.read().unwrap() else {
            panic!("expected a key")
        };
        assert_eq!(k.code, KeyCode::Char('i'));
        assert_eq!(k.modifiers, crate::event::KeyModifiers::CTRL);
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

    /// The constructor seeds `last_size`, so a `SIGWINCH` that does not change
    /// the size must not surface an event. That is what keeps a stray wake on a
    /// recycled pool pipe from being mistaken for a resize.
    #[cfg(not(target_os = "l4re"))]
    #[test]
    fn sigwinch_dedups_unchanged_size() {
        let Some((master, _slave)) = crate::testutil::open_pty_pair() else {
            return;
        };
        // Probe independently of the code under test: illumos ptys are STREAMS
        // devices and a bare master does not answer TIOCGWINSZ. Skipping on the
        // probe rather than on `last_size` keeps the assertion below meaningful
        // everywhere the ioctl does work.
        let ws: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCGWINSZ, &ws) } < 0 {
            return;
        }

        let mut src = new_reader(master);
        assert!(
            src.last_size.is_some(),
            "constructor did not seed the resize dedupe from the input fd"
        );

        unsafe { libc::raise(libc::SIGWINCH) };
        assert!(
            !src.poll(Some(Duration::from_millis(50))).unwrap(),
            "an unchanged size surfaced a resize event"
        );
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

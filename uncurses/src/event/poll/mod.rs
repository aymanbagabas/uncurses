//! Platform-specific readiness wait.
//!
//! Wraps the native readiness primitive on each platform behind a small
//! uniform interface so the event loop in [`super::reader`] does not
//! grow per-target branches. The caller owns a `&mut [PollFd]`:
//! [`Poller::poll`] writes back the `ready` flag on each entry and
//! returns the number of ready fds.
//!
//! Backend selection:
//!
//! | target                                                     | backend                              |
//! |------------------------------------------------------------|--------------------------------------|
//! | `linux`                                                    | `epoll`                              |
//! | `freebsd`, `netbsd`, `openbsd`, `dragonfly`                | `kqueue`                             |
//! | `macos`                                                    | `kqueue` (or `select` on tty fds)    |
//! | `windows`                                                  | `WaitForMultipleObjects`             |
//! | otherwise (`solaris`, `illumos`, generic unix)             | `poll(2)`                            |
//!
//! All backends are level-triggered, treat `HUP`/`ERR`/`EOF` on any
//! watched fd as that fd being ready (so the read path can surface
//! the underlying error), and recompute the remaining timeout across
//! `EINTR` retries from an absolute deadline so callers' timeouts are
//! honoured exactly.

use std::io;
#[cfg(unix)]
use std::os::fd::RawFd;
#[cfg(windows)]
use std::os::windows::io::RawHandle;
use std::time::{Duration, Instant};

/// A handle to wait on plus the readiness bit [`Poller::poll`] writes
/// back. The caller constructs these with [`PollFd::new`], passes a
/// mutable slice to [`Poller::poll`], and reads `ready` per entry on
/// return.
#[derive(Debug, Clone, Copy)]
pub struct PollFd {
    #[cfg(unix)]
    pub fd: RawFd,
    #[cfg(windows)]
    pub fd: RawHandle,
    pub ready: bool,
}

impl PollFd {
    #[cfg(unix)]
    pub fn new(fd: RawFd) -> Self {
        Self { fd, ready: false }
    }
    #[cfg(windows)]
    pub fn new(fd: RawHandle) -> Self {
        Self { fd, ready: false }
    }
}

/// Reset every entry's `ready` flag to `false`. Called by each backend
/// at the top of `poll` so stale bits never leak from a previous call.
#[allow(dead_code)]
fn reset(fds: &mut [PollFd]) {
    for p in fds.iter_mut() {
        p.ready = false;
    }
}

/// Validate the input slice common to every backend.
#[allow(dead_code)]
fn validate(fds: &[PollFd]) -> io::Result<()> {
    if fds.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "poll requires at least one fd",
        ));
    }
    Ok(())
}

/// Convert an absolute deadline into the remaining duration, or `None`
/// for "no deadline / block forever". Already-elapsed deadlines map to
/// `Some(Duration::ZERO)` so the caller polls without blocking.
#[allow(dead_code)] // used by some but not all backends
fn remaining(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|d| {
        let now = Instant::now();
        if d <= now { Duration::ZERO } else { d - now }
    })
}

/// Common contract every backend implements.
///
/// ```ignore
/// let mut p = Poller::new()?;
/// let mut input = PollFd::new(input_fd);
/// let mut wake  = PollFd::new(wake_fd);
/// let n = p.poll(&mut [input, wake], Some(Duration::from_millis(50)))?;
/// ```
///
/// `poll` must:
///
/// * write `ready = true` on every entry whose fd was ready at return
///   time (an all-`false` slice means the wait timed out),
/// * honour the caller's timeout to within syscall resolution,
/// * treat `HUP`/`ERR`/`EOF` on any watched fd as readiness so the
///   read path can surface the underlying error,
/// * loop on `EINTR` against an absolute deadline derived from the
///   first call.
pub trait Poll: Sized {
    fn new() -> io::Result<Self>;
    fn poll(&mut self, fds: &mut [PollFd], timeout: Option<Duration>) -> io::Result<usize>;
}

#[cfg(target_os = "linux")]
pub(crate) mod epoll;

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
))]
pub(crate) mod kqueue;

#[cfg(unix)]
pub(crate) mod poll_sys;
#[cfg(unix)]
pub(crate) mod select;
#[cfg(windows)]
pub(crate) mod windows;

/// Platform-default readiness wait. Picks the best-available backend
/// at compile time. On macOS, swaps between [`kqueue::KqueuePoller`]
/// and [`select::SelectPoller`] on the fly based on whether the
/// current fd slice contains any tty fd (Darwin's kqueue spins on tty
/// character devices).
pub struct Poller {
    inner: Inner,
}

#[cfg(target_os = "linux")]
type Inner = epoll::EpollPoller;

#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
))]
type Inner = kqueue::KqueuePoller;

#[cfg(target_os = "macos")]
enum Inner {
    Kqueue(kqueue::KqueuePoller),
    Select(select::SelectPoller),
}

#[cfg(windows)]
type Inner = windows::WindowsPoller;

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    windows,
)))]
type Inner = poll_sys::PollPoller;

impl Poll for Poller {
    fn new() -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        let inner = Inner::Kqueue(kqueue::KqueuePoller::new()?);
        #[cfg(not(target_os = "macos"))]
        let inner = Inner::new()?;
        Ok(Self { inner })
    }

    fn poll(&mut self, fds: &mut [PollFd], timeout: Option<Duration>) -> io::Result<usize> {
        #[cfg(target_os = "macos")]
        {
            let needs_select = fds.iter().any(|p| unsafe { libc::isatty(p.fd) } == 1);
            match (&self.inner, needs_select) {
                (Inner::Kqueue(_), true) => {
                    self.inner = Inner::Select(select::SelectPoller::new()?);
                }
                (Inner::Select(_), false) => {
                    self.inner = Inner::Kqueue(kqueue::KqueuePoller::new()?);
                }
                _ => {}
            }
            return match &mut self.inner {
                Inner::Kqueue(k) => k.poll(fds, timeout),
                Inner::Select(s) => s.poll(fds, timeout),
            };
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.inner.poll(fds, timeout)
        }
    }
}

// Compile-time assertion that every backend implements the contract.
#[allow(dead_code)]
fn _assert_poll<T: Poll>() {}
#[allow(dead_code)]
fn _assert() {
    _assert_poll::<Poller>();
    #[cfg(unix)]
    _assert_poll::<select::SelectPoller>();
    #[cfg(unix)]
    _assert_poll::<poll_sys::PollPoller>();
    #[cfg(windows)]
    _assert_poll::<windows::WindowsPoller>();
    #[cfg(target_os = "linux")]
    _assert_poll::<epoll::EpollPoller>();
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
    ))]
    _assert_poll::<kqueue::KqueuePoller>();
}

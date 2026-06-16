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

/// A watched handle: a raw fd on unix, a raw `HANDLE` on Windows. The
/// caller registers a slice of these once via [`Poller::new`]; readiness
/// is reported separately by [`Poller::poll`] into a caller-owned
/// `&mut [bool]`, indexed by registration order.
#[cfg(unix)]
pub type PollFd = RawFd;
/// A watched handle: a raw fd on unix, a raw `HANDLE` on Windows. The
/// caller registers a slice of these once via [`Poller::new`]; readiness
/// is reported separately by [`Poller::poll`] into a caller-owned
/// `&mut [bool]`, indexed by registration order.
#[cfg(windows)]
pub type PollFd = RawHandle;

/// Reset every readiness flag to `false`. Called by each backend at the
/// top of `poll` so stale bits never leak from a previous call.
#[allow(dead_code)]
fn reset(ready: &mut [bool]) {
    for r in ready.iter_mut() {
        *r = false;
    }
}

/// Validate the registered fd slice common to every backend: it must be
/// non-empty.
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

/// Validate that the caller's readiness buffer matches the registered fd
/// count, so per-index reporting stays in lockstep.
#[allow(dead_code)]
fn check_ready_len(ready: &[bool], registered: usize) -> io::Result<()> {
    if ready.len() != registered {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "readiness buffer length does not match registered fd count",
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
/// let fds = [input_fd, wake_fd];
/// let p = Epoll::new(&fds)?;
/// let mut ready = [false; 2];
/// let n = p.poll(&mut ready, Some(Duration::from_millis(50)))?;
/// if ready[0] { /* input fd ready */ }
/// ```
///
/// The watched fd set is fixed at construction: [`Poller::new`]
/// registers exactly the fds it is given, once, for the poller's
/// lifetime. Readiness is reported separately into a caller-owned
/// `&mut [bool]` indexed by registration order, so the fds themselves
/// are passed only once. Because registration is done at construction,
/// `poll` takes `&self` and holds no per-call mutable state, so a single
/// poller can be wrapped in [`std::sync::Arc`] (including as
/// `Arc<dyn Poller>`) and waited on concurrently from several threads —
/// the readiness syscalls are thread-safe.
///
/// `poll` must:
///
/// * write `true` into `ready[i]` for every registered fd `i` that was
///   ready at return time (an all-`false` slice means the wait timed
///   out),
/// * honour the caller's timeout to within syscall resolution,
/// * treat `HUP`/`ERR`/`EOF` on any watched fd as readiness so the
///   read path can surface the underlying error,
/// * loop on `EINTR` against an absolute deadline derived from the
///   first call.
///
/// There is deliberately no platform-dispatch wrapper: the concrete
/// implementations are exposed per target and the caller picks one. On
/// Darwin, `kqueue` spins on tty character devices, so a caller watching
/// a tty input fd selects [`Select`] over [`Kqueue`].
pub trait Poller: Send + Sync {
    /// Construct a poller watching exactly `fds`, registered once for
    /// the poller's lifetime.
    fn new(fds: &[PollFd]) -> io::Result<Self>
    where
        Self: Sized;
    /// Wait until one of the registered fds is ready or `timeout`
    /// elapses, writing readiness into `ready` (indexed by the order the
    /// fds were registered; its length must equal the registered fd
    /// count). Takes `&self` so the poller can be shared and waited on
    /// concurrently.
    fn poll(&self, ready: &mut [bool], timeout: Option<Duration>) -> io::Result<usize>;
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
#[allow(clippy::module_inception)]
pub(crate) mod poll;
#[cfg(unix)]
pub(crate) mod select;
#[cfg(windows)]
pub(crate) mod windows;

#[cfg(target_os = "linux")]
pub(crate) use epoll::Epoll;
#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
))]
pub(crate) use kqueue::Kqueue;
#[cfg(unix)]
pub(crate) use poll::Poll;
#[cfg(unix)]
pub(crate) use select::Select;
#[cfg(windows)]
pub(crate) use windows::Windows;

/// Construct the platform's default poller watching `fds`, as a shared
/// trait object. `input_is_tty` only matters on Darwin, where `kqueue`
/// spins on tty character devices: a tty input fd selects [`Select`],
/// otherwise [`Kqueue`]. The choice is made once here, at construction,
/// so the returned poller's `poll` stays stateless and `&self`.
#[allow(unused_variables)]
pub(crate) fn new_poller(
    fds: &[PollFd],
    input_is_tty: bool,
) -> io::Result<std::sync::Arc<dyn Poller>> {
    #[cfg(target_os = "linux")]
    {
        Ok(std::sync::Arc::new(Epoll::new(fds)?))
    }
    #[cfg(target_os = "macos")]
    {
        if input_is_tty {
            Ok(std::sync::Arc::new(Select::new(fds)?))
        } else {
            Ok(std::sync::Arc::new(Kqueue::new(fds)?))
        }
    }
    #[cfg(any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
    ))]
    {
        Ok(std::sync::Arc::new(Kqueue::new(fds)?))
    }
    #[cfg(windows)]
    {
        Ok(std::sync::Arc::new(Windows::new(fds)?))
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
        windows,
    )))]
    {
        Ok(std::sync::Arc::new(Poll::new(fds)?))
    }
}

// Compile-time assertion that every backend implements the contract and
// is usable as a shared trait object.
#[allow(dead_code)]
fn _assert_poller<T: Poller>() {}
#[allow(dead_code)]
fn _assert_obj(_: &dyn Poller) {}
#[allow(dead_code)]
fn _assert() {
    #[cfg(unix)]
    _assert_poller::<Select>();
    #[cfg(unix)]
    _assert_poller::<Poll>();
    #[cfg(windows)]
    _assert_poller::<Windows>();
    #[cfg(target_os = "linux")]
    _assert_poller::<Epoll>();
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
    ))]
    _assert_poller::<Kqueue>();
}

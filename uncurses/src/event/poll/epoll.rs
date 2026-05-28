//! Linux `epoll` readiness backend.
//!
//! Holds a long-lived epoll instance so registrations persist across
//! `poll` calls. Each call diffs the caller's `&mut [PollFd]` against
//! the currently-registered set, adds any new fds and removes any that
//! are gone. All fds are watched in level-triggered mode (so the
//! source's existing generic `Read` path keeps working against
//! potentially blocking fds) and `EPOLLHUP | EPOLLERR` fold into
//! readiness so the read path surfaces the actual error.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

use super::{Poll, PollFd, remaining, reset, validate};

pub(crate) struct EpollPoller {
    epfd: OwnedFd,
    registered: Vec<RawFd>,
}

impl EpollPoller {
    /// Reconcile `self.registered` with the caller's slice: add any
    /// fd in `fds` that isn't registered, remove any registered fd
    /// no longer present.
    fn sync(&mut self, fds: &[PollFd]) -> io::Result<()> {
        let mut i = 0;
        while i < self.registered.len() {
            let fd = self.registered[i];
            if !fds.iter().any(|p| p.fd == fd) {
                // EPOLL_CTL_DEL is best-effort: an already-closed fd
                // would return ENOENT/EBADF and we still want it gone
                // from our bookkeeping.
                unsafe {
                    libc::epoll_ctl(
                        self.epfd.as_raw_fd(),
                        libc::EPOLL_CTL_DEL,
                        fd,
                        std::ptr::null_mut(),
                    );
                }
                self.registered.swap_remove(i);
            } else {
                i += 1;
            }
        }
        for p in fds {
            if !self.registered.contains(&p.fd) {
                let mut ev = libc::epoll_event {
                    events: (libc::EPOLLIN | libc::EPOLLERR | libc::EPOLLHUP) as u32,
                    u64: fd_to_u64(p.fd),
                };
                let rc = unsafe {
                    libc::epoll_ctl(self.epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, p.fd, &mut ev)
                };
                if rc < 0 {
                    return Err(io::Error::last_os_error());
                }
                self.registered.push(p.fd);
            }
        }
        Ok(())
    }
}

impl Poll for EpollPoller {
    fn new() -> io::Result<Self> {
        let raw = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: epoll_create1 just returned a fresh owned fd.
        let epfd = unsafe { OwnedFd::from_raw_fd(raw) };
        Ok(Self {
            epfd,
            registered: Vec::new(),
        })
    }

    fn poll(&mut self, fds: &mut [PollFd], timeout: Option<Duration>) -> io::Result<usize> {
        validate(fds)?;
        reset(fds);
        self.sync(fds)?;

        let deadline = timeout.map(|t| Instant::now() + t);
        let mut events: Vec<libc::epoll_event> = vec![unsafe { std::mem::zeroed() }; fds.len()];
        loop {
            let ms = match remaining(deadline) {
                None => -1i32,
                Some(d) => duration_to_ms(d),
            };
            let n = unsafe {
                libc::epoll_wait(
                    self.epfd.as_raw_fd(),
                    events.as_mut_ptr(),
                    events.len() as i32,
                    ms,
                )
            };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            let mut count = 0usize;
            for ev in &events[..n as usize] {
                let fd = u64_to_fd(ev.u64);
                if let Some(p) = fds.iter_mut().find(|p| p.fd == fd)
                    && !p.ready
                {
                    p.ready = true;
                    count += 1;
                }
            }
            return Ok(count);
        }
    }
}

fn duration_to_ms(d: Duration) -> i32 {
    let ms = d.as_millis();
    if ms > i32::MAX as u128 {
        i32::MAX
    } else {
        ms as i32
    }
}

// `RawFd` is `i32`. Zero-extend through `u32` so the round-trip is
// lossless even for negative-cast values.
fn fd_to_u64(fd: RawFd) -> u64 {
    fd as u32 as u64
}

fn u64_to_fd(v: u64) -> RawFd {
    v as u32 as i32
}

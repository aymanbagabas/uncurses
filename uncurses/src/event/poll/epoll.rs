//! Linux `epoll` readiness backend.
//!
//! ## Purpose
//!
//! [`Epoll`] registers the source's fixed fd set once and then waits for
//! level-triggered read readiness. Registration indices are stored in the epoll
//! event token so readiness maps directly back into the caller's boolean slice.
//!
//! ## Gotchas
//!
//! `EPOLLERR` and `EPOLLHUP` are reported as readiness; the subsequent read path
//! is responsible for surfacing the actual EOF or error. Timeouts are recomputed
//! across `EINTR` from an absolute deadline.
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use super::{PollFd, Poller, check_ready_len, remaining, reset, validate};

pub(crate) struct Epoll {
    epfd: OwnedFd,
    count: usize,
}

impl Poller for Epoll {
    fn new(fds: &[PollFd]) -> io::Result<Self> {
        validate(fds)?;
        let raw = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: epoll_create1 just returned a fresh owned fd.
        let epfd = unsafe { OwnedFd::from_raw_fd(raw) };
        for (i, &fd) in fds.iter().enumerate() {
            // Carry the registration index in the token so `poll` maps
            // a ready event straight to `ready[i]`.
            let mut ev = libc::epoll_event {
                events: (libc::EPOLLIN | libc::EPOLLERR | libc::EPOLLHUP) as u32,
                u64: i as u64,
            };
            let rc = unsafe { libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, fd, &mut ev) };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self {
            epfd,
            count: fds.len(),
        })
    }

    fn poll(&self, ready: &mut [bool], timeout: Option<Duration>) -> io::Result<usize> {
        check_ready_len(ready, self.count)?;
        reset(ready);

        let deadline = timeout.map(|t| Instant::now() + t);
        let mut events: Vec<libc::epoll_event> = vec![unsafe { std::mem::zeroed() }; self.count];
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
                let i = ev.u64 as usize;
                if let Some(slot) = ready.get_mut(i)
                    && !*slot
                {
                    *slot = true;
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

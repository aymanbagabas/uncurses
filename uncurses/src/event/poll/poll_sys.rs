//! `poll(2)` readiness backend.
//!
//! Stateless: each [`PollPoller::poll`] call rebuilds a fresh
//! `pollfd` array from the caller's slice. `POLLIN | POLLHUP | POLLERR
//! | POLLNVAL` all fold into readiness so the read path surfaces the
//! underlying error.

use std::io;
use std::time::{Duration, Instant};

use super::{Poll, PollFd, remaining, reset, validate};

pub(crate) struct PollPoller;

impl Poll for PollPoller {
    fn new() -> io::Result<Self> {
        Ok(Self)
    }

    fn poll(&mut self, fds: &mut [PollFd], timeout: Option<Duration>) -> io::Result<usize> {
        validate(fds)?;
        reset(fds);

        let deadline = timeout.map(|t| Instant::now() + t);
        let mut pfds: Vec<libc::pollfd> = fds
            .iter()
            .map(|p| libc::pollfd {
                fd: p.fd,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        loop {
            let ms = match remaining(deadline) {
                None => -1i32,
                Some(d) => duration_to_ms(d),
            };
            let rc = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, ms) };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            let mut count = 0usize;
            let mask = libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL;
            for (out, pfd) in fds.iter_mut().zip(pfds.iter()) {
                if (pfd.revents & mask) != 0 {
                    out.ready = true;
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

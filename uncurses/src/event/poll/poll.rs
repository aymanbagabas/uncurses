//! `poll(2)` readiness backend.
//!
//! Stores the registered fds and rebuilds a fresh `pollfd` array each
//! [`Poll::poll`] call. `POLLIN | POLLHUP | POLLERR | POLLNVAL` all fold
//! into readiness so the read path surfaces the underlying error.

use std::io;
use std::time::{Duration, Instant};

use super::{PollFd, Poller, check_ready_len, remaining, reset, validate};

pub(crate) struct Poll {
    fds: Vec<PollFd>,
}

impl Poller for Poll {
    fn new(fds: &[PollFd]) -> io::Result<Self> {
        validate(fds)?;
        Ok(Self { fds: fds.to_vec() })
    }

    fn poll(&self, ready: &mut [bool], timeout: Option<Duration>) -> io::Result<usize> {
        check_ready_len(ready, self.fds.len())?;
        reset(ready);

        let deadline = timeout.map(|t| Instant::now() + t);
        let mut pfds: Vec<libc::pollfd> = self
            .fds
            .iter()
            .map(|&fd| libc::pollfd {
                fd,
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
            for (slot, pfd) in ready.iter_mut().zip(pfds.iter()) {
                if (pfd.revents & mask) != 0 {
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

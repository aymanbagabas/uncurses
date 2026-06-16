//! `select(2)` readiness backend.
//!
//! Stores the registered fds and rebuilds the `fd_set` each
//! [`Select::poll`] call. Subject to the usual `FD_SETSIZE` cap; the
//! backend rejects fds that are out of range with `InvalidInput`.

use std::time::{Duration, Instant};
use std::{io, os::fd::RawFd};

use super::{PollFd, Poller, check_ready_len, remaining, reset, validate};

pub(crate) struct Select {
    fds: Vec<PollFd>,
}

fn check_fd_range(fds: &[PollFd]) -> io::Result<()> {
    for &fd in fds.iter() {
        if fd < 0 || fd >= libc::FD_SETSIZE as RawFd {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fd exceeds FD_SETSIZE for select(2) backend",
            ));
        }
    }
    Ok(())
}

impl Poller for Select {
    fn new(fds: &[PollFd]) -> io::Result<Self> {
        validate(fds)?;
        check_fd_range(fds)?;
        Ok(Self { fds: fds.to_vec() })
    }

    fn poll(&self, ready: &mut [bool], timeout: Option<Duration>) -> io::Result<usize> {
        check_ready_len(ready, self.fds.len())?;
        reset(ready);

        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let mut rfds: libc::fd_set = unsafe { std::mem::zeroed() };
            let mut nfds: RawFd = 0;
            for &fd in self.fds.iter() {
                unsafe { libc::FD_SET(fd, &mut rfds) };
                if fd + 1 > nfds {
                    nfds = fd + 1;
                }
            }

            let mut tv_storage;
            let tv_ptr: *mut libc::timeval = match remaining(deadline) {
                None => std::ptr::null_mut(),
                Some(d) => {
                    tv_storage = libc::timeval {
                        tv_sec: d.as_secs() as _,
                        tv_usec: d.subsec_micros() as _,
                    };
                    &raw mut tv_storage
                }
            };

            let rc = unsafe {
                libc::select(
                    nfds,
                    &mut rfds,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    tv_ptr,
                )
            };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            let mut count = 0usize;
            for (slot, &fd) in ready.iter_mut().zip(self.fds.iter()) {
                if unsafe { libc::FD_ISSET(fd, &rfds) } {
                    *slot = true;
                    count += 1;
                }
            }
            return Ok(count);
        }
    }
}

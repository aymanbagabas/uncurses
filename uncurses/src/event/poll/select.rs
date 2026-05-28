//! `select(2)` readiness backend.
//!
//! Stateless: each [`SelectPoller::poll`] call rebuilds the `fd_set`
//! from the caller's slice. Subject to the usual `FD_SETSIZE` cap; the
//! backend rejects fds that are out of range with `InvalidInput`.

use std::time::{Duration, Instant};
use std::{io, os::fd::RawFd};

use super::{Poll, PollFd, remaining, reset, validate};

pub(crate) struct SelectPoller;

impl Poll for SelectPoller {
    fn new() -> io::Result<Self> {
        Ok(Self)
    }

    fn poll(&mut self, fds: &mut [PollFd], timeout: Option<Duration>) -> io::Result<usize> {
        validate(fds)?;
        for p in fds.iter() {
            if p.fd < 0 || p.fd >= libc::FD_SETSIZE as RawFd {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "fd exceeds FD_SETSIZE for select(2) backend",
                ));
            }
        }
        reset(fds);

        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let mut rfds: libc::fd_set = unsafe { std::mem::zeroed() };
            let mut nfds: RawFd = 0;
            for p in fds.iter() {
                unsafe { libc::FD_SET(p.fd, &mut rfds) };
                if p.fd + 1 > nfds {
                    nfds = p.fd + 1;
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
            for p in fds.iter_mut() {
                if unsafe { libc::FD_ISSET(p.fd, &rfds) } {
                    p.ready = true;
                    count += 1;
                }
            }
            return Ok(count);
        }
    }
}

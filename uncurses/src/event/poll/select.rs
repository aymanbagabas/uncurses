//! `select(2)` readiness backend.
//!
//! ## Purpose
//!
//! [`Select`] is the portable Unix fallback and the macOS tty-input backend. It
//! stores the fixed fd list and rebuilds the read set for each wait.
//!
//! ## Platform paths
//!
//! * macOS calls the unbounded `select$DARWIN_EXTSN` symbol and manages the bit
//!   buffer manually so fds above `FD_SETSIZE` are safe.
//! * Other Unix targets use `libc::fd_set` and reject fds outside `FD_SETSIZE`
//!   at construction.
//!
//! ## Gotchas
//!
//! `select` mutates its fd set and timeout arguments, so both are rebuilt for
//! each retry after `EINTR`. The backend reports only read readiness; hangups
//! appear as readable and are handled by the source read path.
use std::time::{Duration, Instant};
use std::{io, os::fd::RawFd};

use super::{PollFd, Poller, check_ready_len, remaining, reset, validate};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// `select(2)` bound to the variant that is not limited by
    /// `FD_SETSIZE` — the same symbol the C macro
    /// `_DARWIN_UNLIMITED_SELECT` selects. Declared directly here,
    /// mirroring how `libc` itself binds other `$DARWIN_EXTSN` functions
    /// (e.g. `realpath`, `syslog`) across macOS architectures.
    #[link_name = "select$DARWIN_EXTSN"]
    fn select_unlimited(
        nfds: libc::c_int,
        readfds: *mut libc::fd_set,
        writefds: *mut libc::fd_set,
        errorfds: *mut libc::fd_set,
        timeout: *mut libc::timeval,
    ) -> libc::c_int;
}

pub(crate) struct Select {
    fds: Vec<PollFd>,
}

#[cfg(not(target_os = "macos"))]
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
        #[cfg(not(target_os = "macos"))]
        check_fd_range(fds)?;
        Ok(Self { fds: fds.to_vec() })
    }

    fn poll(&self, ready: &mut [bool], timeout: Option<Duration>) -> io::Result<usize> {
        check_ready_len(ready, self.fds.len())?;
        reset(ready);
        #[cfg(target_os = "macos")]
        {
            self.poll_darwin(ready, timeout)
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.poll_unix(ready, timeout)
        }
    }
}

impl Select {
    /// Darwin path: hand-managed, `FD_SETSIZE`-unbounded read set.
    #[cfg(target_os = "macos")]
    fn poll_darwin(&self, ready: &mut [bool], timeout: Option<Duration>) -> io::Result<usize> {
        // Darwin `fd_set` is an array of 32-bit words (`__int32_t`), one
        // bit per fd. Size the buffer to the largest watched fd so a high
        // fd never overflows it.
        const NFDBITS: RawFd = (std::mem::size_of::<i32>() * 8) as RawFd; // 32

        let max_fd = self.fds.iter().copied().max().unwrap_or(0);
        let words = (max_fd as usize / NFDBITS as usize) + 1;
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            // `select` mutates the set in place, so it is rebuilt each
            // iteration; a fresh buffer also keeps `poll(&self)` free of
            // shared mutable state.
            let mut bits = vec![0i32; words];
            for &fd in self.fds.iter() {
                bits[fd as usize / NFDBITS as usize] |= 1 << (fd % NFDBITS);
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
                select_unlimited(
                    max_fd + 1,
                    bits.as_mut_ptr() as *mut libc::fd_set,
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
                if bits[fd as usize / NFDBITS as usize] & (1 << (fd % NFDBITS)) != 0 {
                    *slot = true;
                    count += 1;
                }
            }
            return Ok(count);
        }
    }

    /// Generic unix path: fixed-size `libc::fd_set`, bounded by
    /// `FD_SETSIZE` (enforced at construction).
    #[cfg(not(target_os = "macos"))]
    fn poll_unix(&self, ready: &mut [bool], timeout: Option<Duration>) -> io::Result<usize> {
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Raise the soft fd limit so a watched fd can be placed above
    /// `FD_SETSIZE` (capped at the hard limit).
    fn raise_nofile(target: u64) {
        unsafe {
            let mut rl: libc::rlimit = std::mem::zeroed();
            assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut rl), 0);
            if rl.rlim_cur < target {
                rl.rlim_cur = target.min(rl.rlim_max);
                assert_eq!(
                    libc::setrlimit(libc::RLIMIT_NOFILE, &rl),
                    0,
                    "raise RLIMIT_NOFILE"
                );
            }
        }
    }

    // A watched fd at or above FD_SETSIZE (1024) must be registrable and
    // report readiness — the whole point of the hand-managed fd_set. The
    // fixed-size path would reject it at construction.
    #[test]
    fn select_watches_fd_above_fd_setsize() {
        const HIGH: RawFd = 2000;
        assert!(HIGH >= libc::FD_SETSIZE as RawFd);
        raise_nofile(HIGH as u64 + 16);

        // Make a pipe, then move its read end to a high fd number.
        let mut fds = [0 as RawFd; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
        let (r, w) = (fds[0], fds[1]);
        assert!(unsafe { libc::dup2(r, HIGH) } >= 0, "dup2 to high fd");
        unsafe { libc::close(r) };

        let poller = Select::new(&[HIGH]).expect("construct select on high fd");

        // Nothing buffered: the wait times out.
        let mut ready = [false];
        assert_eq!(
            poller
                .poll(&mut ready, Some(Duration::from_millis(10)))
                .unwrap(),
            0
        );
        assert!(!ready[0]);

        // After a write the high fd reports readable.
        let b = [b'x'];
        assert_eq!(
            unsafe { libc::write(w, b.as_ptr() as *const _, 1) },
            1,
            "write"
        );
        let mut ready = [false];
        assert_eq!(
            poller
                .poll(&mut ready, Some(Duration::from_millis(500)))
                .unwrap(),
            1
        );
        assert!(ready[0]);

        unsafe {
            libc::close(HIGH);
            libc::close(w);
        }
    }
}

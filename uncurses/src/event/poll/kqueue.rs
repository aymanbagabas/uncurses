//! BSD/macOS `kqueue` readiness backend.
//!
//! Registers its watched fds once at construction and keeps the kqueue
//! for its lifetime, so [`Kqueue::poll`] only waits — it takes `&self`
//! and holds no mutable state, which lets the poller be shared (e.g.
//! behind `Arc`) and waited on concurrently. Events are registered with
//! `EVFILT_READ` without `EV_CLEAR` (i.e. level-triggered) and `EV_EOF`
//! folds into readiness so the read path surfaces the underlying error.
//!
//! Note: on macOS, `EVFILT_READ` against a tty character device returns
//! immediately with `data == 0` in a tight loop. A caller watching a tty
//! input fd on Darwin should use [`super::Select`] instead.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use super::{PollFd, Poller, check_ready_len, remaining, reset, validate};

pub(crate) struct Kqueue {
    kq: OwnedFd,
    count: usize,
}

impl Poller for Kqueue {
    fn new(fds: &[PollFd]) -> io::Result<Self> {
        validate(fds)?;
        let raw = unsafe { libc::kqueue() };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: kqueue() returns a fresh owned fd on success.
        let kq = unsafe { OwnedFd::from_raw_fd(raw) };
        // Mark the kqueue fd close-on-exec since kqueue() itself does
        // not set FD_CLOEXEC on any current BSD.
        unsafe {
            let flags = libc::fcntl(kq.as_raw_fd(), libc::F_GETFD);
            if flags >= 0 {
                let _ = libc::fcntl(kq.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
        for (i, &fd) in fds.iter().enumerate() {
            // Carry the registration index in `udata` so `poll` maps a
            // ready event straight to `ready[i]`.
            let ev = libc::kevent {
                ident: fd as usize,
                filter: libc::EVFILT_READ,
                flags: libc::EV_ADD,
                fflags: 0,
                data: 0,
                udata: index_to_udata(i),
            };
            let rc = unsafe {
                libc::kevent(
                    kq.as_raw_fd(),
                    &ev,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self {
            kq,
            count: fds.len(),
        })
    }

    fn poll(&self, ready: &mut [bool], timeout: Option<Duration>) -> io::Result<usize> {
        check_ready_len(ready, self.count)?;
        reset(ready);

        let deadline = timeout.map(|t| Instant::now() + t);
        let mut events: Vec<libc::kevent> = (0..self.count)
            .map(|_| unsafe { std::mem::zeroed() })
            .collect();
        loop {
            let ts_storage;
            let ts_ptr: *const libc::timespec = match remaining(deadline) {
                None => std::ptr::null(),
                Some(d) => {
                    ts_storage = libc::timespec {
                        tv_sec: d.as_secs() as _,
                        tv_nsec: d.subsec_nanos() as _,
                    };
                    &ts_storage
                }
            };
            let n = unsafe {
                libc::kevent(
                    self.kq.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    events.len() as _,
                    ts_ptr,
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
                let i = udata_to_index(ev.udata);
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

// Round-trip a registration index through the `*mut c_void` udata field.
fn index_to_udata(i: usize) -> *mut libc::c_void {
    i as *mut libc::c_void
}

fn udata_to_index(p: *mut libc::c_void) -> usize {
    p as usize
}

//! BSD/macOS `kqueue` readiness backend.
//!
//! Holds a long-lived kqueue so registrations persist across `poll`
//! calls. Each call diffs the caller's `&mut [PollFd]` against the
//! currently-registered set and adds or removes fds as needed. Events
//! are registered with `EVFILT_READ` without `EV_CLEAR` (i.e.
//! level-triggered) and `EV_EOF` folds into readiness so the read
//! path surfaces the underlying error.
//!
//! Note: on macOS, `EVFILT_READ` against a tty character device
//! returns immediately with `data == 0` in a tight loop. Callers that
//! may watch tty fds on Darwin should use the top-level [`super::Poll`]
//! wrapper, which transparently falls back to
//! [`super::select::SelectPoller`] in that case.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

use super::{Poll, PollFd, remaining, reset, validate};

pub(crate) struct KqueuePoller {
    kq: OwnedFd,
    registered: Vec<RawFd>,
}

impl KqueuePoller {
    fn add(kq: RawFd, fd: RawFd) -> io::Result<()> {
        let ev = libc::kevent {
            ident: fd as usize,
            filter: libc::EVFILT_READ,
            flags: libc::EV_ADD,
            fflags: 0,
            data: 0,
            udata: fd_to_udata(fd),
        };
        let rc = unsafe { libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn del(kq: RawFd, fd: RawFd) {
        let ev = libc::kevent {
            ident: fd as usize,
            filter: libc::EVFILT_READ,
            flags: libc::EV_DELETE,
            fflags: 0,
            data: 0,
            udata: fd_to_udata(fd),
        };
        // Best-effort: a closed fd is already gone from the kqueue.
        unsafe {
            libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null());
        }
    }

    fn sync(&mut self, fds: &[PollFd]) -> io::Result<()> {
        let kq = self.kq.as_raw_fd();
        let mut i = 0;
        while i < self.registered.len() {
            let fd = self.registered[i];
            if !fds.iter().any(|p| p.fd == fd) {
                Self::del(kq, fd);
                self.registered.swap_remove(i);
            } else {
                i += 1;
            }
        }
        for p in fds {
            if !self.registered.contains(&p.fd) {
                Self::add(kq, p.fd)?;
                self.registered.push(p.fd);
            }
        }
        Ok(())
    }
}

impl Poll for KqueuePoller {
    fn new() -> io::Result<Self> {
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
        Ok(Self {
            kq,
            registered: Vec::new(),
        })
    }

    fn poll(&mut self, fds: &mut [PollFd], timeout: Option<Duration>) -> io::Result<usize> {
        validate(fds)?;
        reset(fds);
        self.sync(fds)?;

        let deadline = timeout.map(|t| Instant::now() + t);
        let mut events: Vec<libc::kevent> = (0..fds.len())
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
                let fd = udata_to_fd(ev.udata);
                if let Some(p) = fds.iter_mut().find(|p| p.fd == fd) {
                    if !p.ready {
                        p.ready = true;
                        count += 1;
                    }
                }
            }
            return Ok(count);
        }
    }
}

// Round-trip an `i32` fd through a `*mut c_void` udata field
// losslessly: zero-extend through `u32` so negative-cast values
// survive.
fn fd_to_udata(fd: RawFd) -> *mut libc::c_void {
    (fd as u32 as usize) as *mut libc::c_void
}

fn udata_to_fd(p: *mut libc::c_void) -> RawFd {
    (p as usize) as u32 as i32
}

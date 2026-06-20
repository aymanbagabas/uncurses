//! Windows `WaitForMultipleObjects` readiness backend.
//!
//! ## Purpose
//!
//! [`Windows`] waits on the console input handle and wake event registered by
//! [`EventSource`](crate::event::EventSource). It reports every handle that is
//! signaled at return time by probing the remaining handles with zero-timeout
//! waits after the first signal.
//!
//! ## Gotchas
//!
//! `WaitForMultipleObjects` supports at most 64 handles; construction rejects
//! larger sets. The poller borrows raw handles owned by the source and must not
//! outlive them.
use std::io;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{WaitForMultipleObjects, WaitForSingleObject};

use super::{PollFd, Poller, check_ready_len, remaining, reset, validate};

const INFINITE: u32 = u32::MAX;
/// `WaitForMultipleObjects` accepts up to `MAXIMUM_WAIT_OBJECTS` (64) handles.
const MAX_WAIT: usize = 64;

pub struct Windows {
    handles: Vec<HANDLE>,
}

// `HANDLE` is a raw pointer; the watched handles are owned elsewhere for
// the poller's lifetime and only waited on, never dereferenced, so the
// poller is safe to share across threads.
unsafe impl Send for Windows {}
unsafe impl Sync for Windows {}

impl Poller for Windows {
    fn new(fds: &[PollFd]) -> io::Result<Self> {
        validate(fds)?;
        if fds.len() > MAX_WAIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "too many handles for WaitForMultipleObjects",
            ));
        }
        Ok(Self {
            handles: fds.iter().map(|&h| h as HANDLE).collect(),
        })
    }

    fn poll(&self, ready: &mut [bool], timeout: Option<Duration>) -> io::Result<usize> {
        check_ready_len(ready, self.handles.len())?;
        reset(ready);

        let deadline = timeout.map(|t| Instant::now() + t);
        let n = self.handles.len() as u32;

        let ms = duration_to_ms(remaining(deadline).or(timeout));
        let rc = unsafe { WaitForMultipleObjects(n, self.handles.as_ptr(), 0, ms) };
        if rc == WAIT_TIMEOUT {
            return Ok(0);
        }
        if rc == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        }
        if rc >= WAIT_OBJECT_0 + n {
            return Err(io::Error::other(format!(
                "wait returned unexpected status {rc}"
            )));
        }

        let first = (rc - WAIT_OBJECT_0) as usize;
        ready[first] = true;
        let mut count = 1usize;
        // Probe remaining handles with zero-timeout single waits so all
        // simultaneously signaled handles are reported in one call.
        for (i, &h) in self.handles.iter().enumerate() {
            if i == first {
                continue;
            }
            let rc2 = unsafe { WaitForSingleObject(h, 0) };
            if rc2 == WAIT_OBJECT_0 {
                ready[i] = true;
                count += 1;
            }
        }
        Ok(count)
    }
}

fn duration_to_ms(timeout: Option<Duration>) -> u32 {
    match timeout {
        None => INFINITE,
        Some(d) => {
            let ms = d.as_millis();
            if ms >= INFINITE as u128 {
                INFINITE - 1
            } else {
                ms as u32
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_to_ms_caps_at_infinite_minus_one() {
        let huge = Duration::from_secs(u64::MAX / 2);
        assert!(duration_to_ms(Some(huge)) < INFINITE);
        assert_eq!(duration_to_ms(None), INFINITE);
        assert_eq!(duration_to_ms(Some(Duration::from_millis(0))), 0);
        assert_eq!(duration_to_ms(Some(Duration::from_millis(50))), 50);
    }
}

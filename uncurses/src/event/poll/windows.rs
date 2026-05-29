//! `WaitForMultipleObjects`-backed [`Poll`] implementation.

use std::io;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{WaitForMultipleObjects, WaitForSingleObject};

use super::{Poll, PollFd, remaining, reset, validate};

const INFINITE: u32 = u32::MAX;
/// `WaitForMultipleObjects` accepts up to `MAXIMUM_WAIT_OBJECTS` (64) handles.
const MAX_WAIT: usize = 64;

pub struct WindowsPoller;

impl Poll for WindowsPoller {
    fn new() -> io::Result<Self> {
        Ok(Self)
    }

    fn poll(&mut self, fds: &mut [PollFd], timeout: Option<Duration>) -> io::Result<usize> {
        validate(fds)?;
        reset(fds);
        if fds.len() > MAX_WAIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "too many handles for WaitForMultipleObjects",
            ));
        }

        let deadline = timeout.map(|t| Instant::now() + t);
        let mut handles: [HANDLE; MAX_WAIT] = [0 as HANDLE; MAX_WAIT];
        for (i, p) in fds.iter().enumerate() {
            handles[i] = p.fd as HANDLE;
        }
        let n = fds.len() as u32;

        let ms = duration_to_ms(remaining(deadline).or(timeout));
        let rc = unsafe { WaitForMultipleObjects(n, handles.as_ptr(), 0, ms) };
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
        fds[first].ready = true;
        let mut ready = 1usize;
        // Probe remaining handles with zero-timeout single waits so all
        // simultaneously signaled handles are reported in one call.
        for (i, p) in fds.iter_mut().enumerate() {
            if i == first {
                continue;
            }
            let rc2 = unsafe { WaitForSingleObject(p.fd as HANDLE, 0) };
            if rc2 == WAIT_OBJECT_0 {
                p.ready = true;
                ready += 1;
            }
        }
        Ok(ready)
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

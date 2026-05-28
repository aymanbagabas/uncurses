//! SIGWINCH (window-resize) notification.
//!
//! Installs a single shared SIGWINCH handler and exposes a wakeable
//! [`subscribe`] API: each subscriber registers a file descriptor and
//! the handler writes a single byte to it on every resize, so a
//! `poll`/`select`/`epoll` waiting on that fd unblocks immediately.
//!
//! The signal handler only performs async-signal-safe operations
//! (atomic updates and `write(2)` on already-open fds).

#[cfg(unix)]
use std::sync::atomic::{AtomicI32, Ordering};

#[cfg(unix)]
static INSTALLED: std::sync::Once = std::sync::Once::new();

#[cfg(unix)]
const MAX_SUBSCRIBERS: usize = 32;

/// Slot table for `subscribe`. `-1` means "free"; any non-negative value
/// is a borrowed fd belonging to a live [`Subscription`]. Reads and writes
/// from the signal handler use [`Ordering::Relaxed`] — visibility of newer
/// registrations is established by the AcqRel exchange in [`subscribe`].
#[cfg(unix)]
static SUBSCRIBERS: [AtomicI32; MAX_SUBSCRIBERS] = [const { AtomicI32::new(-1) }; MAX_SUBSCRIBERS];

#[cfg(unix)]
extern "C" fn handler(_sig: libc::c_int) {
    // Notify every registered subscriber. write(2) is async-signal-safe
    // (POSIX.1-2017, Sec. 2.4.3).
    for slot in SUBSCRIBERS.iter() {
        let fd = slot.load(Ordering::Relaxed);
        if fd >= 0 {
            let buf = [b'w'];
            // SAFETY: fd was registered by a live Subscription whose Drop
            // unregisters before closing the descriptor.
            unsafe {
                let _ = libc::write(fd, buf.as_ptr() as *const _, 1);
            }
        }
    }
}

#[cfg(unix)]
fn install_handler() {
    INSTALLED.call_once(|| unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as *const () as usize;
        // Restart syscalls if possible; do not block other signals.
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGWINCH, &sa, std::ptr::null_mut());
    });
}

/// RAII handle that unregisters its slot on drop.
#[cfg(unix)]
#[derive(Debug)]
pub(crate) struct Subscription {
    slot: usize,
}

#[cfg(unix)]
impl Drop for Subscription {
    fn drop(&mut self) {
        // Release ordering pairs with the AcqRel in `subscribe` so that any
        // subsequent registrant observes the slot as free.
        SUBSCRIBERS[self.slot].store(-1, Ordering::Release);
    }
}

/// Register `fd` to receive a one-byte wake from the SIGWINCH handler on
/// every resize. Returns an RAII [`Subscription`]; drop it (or the source
/// owning it) to unsubscribe before closing `fd`.
///
/// `fd` is borrowed — callers must keep it open for the lifetime of the
/// returned [`Subscription`] and close it only after the subscription is
/// dropped.
#[cfg(unix)]
pub(crate) fn subscribe(fd: i32) -> std::io::Result<Subscription> {
    install_handler();
    for (i, slot) in SUBSCRIBERS.iter().enumerate() {
        if slot
            .compare_exchange(-1, fd, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Ok(Subscription { slot: i });
        }
    }
    Err(std::io::Error::other("too many SIGWINCH subscribers"))
}

#[cfg(not(unix))]
#[derive(Debug)]
pub(crate) struct Subscription;

#[cfg(not(unix))]
pub(crate) fn subscribe(_fd: i32) -> std::io::Result<Subscription> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "SIGWINCH not supported on this platform",
    ))
}

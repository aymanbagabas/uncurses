//! Controlling-terminal helpers.
//!
//! [`open_tty`] opens the controlling terminal directly, useful when
//! stdio is piped or redirected but the program still needs to talk to
//! a real terminal. On Unix both halves of the returned pair refer to
//! `/dev/tty`; on Windows the input is `CONIN$` and the output is
//! `CONOUT$`.
//!
//! The pair is cached process-wide on the first successful call, so
//! [`open_tty`] can be called freely from multiple sites without
//! reopening the device. Both [`TtyInput`] and [`TtyOutput`] are
//! [`Copy`] handles that reference the shared cache and serialise
//! concurrent reads / writes through a [`Mutex`].
//!
//! The Windows [`TtyOutput`] `Write` impl transparently routes console
//! writes through `WriteConsoleW` (UTF-16) so non-ASCII text
//! round-trips correctly through the conpty, matching
//! [`std::io::Stdout`].

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::sync::{Mutex, OnceLock};

#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};

#[cfg(windows)]
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, RawHandle};

// ---------------------------------------------------------------------------
// Process-wide singletons
//
// Both halves of the controlling-terminal pair are cached the first
// time [`open_tty`] is called. Subsequent calls return new `Copy`
// handles that reference the same underlying [`File`] guarded by a
// [`Mutex`], so concurrent writes from multiple threads are serialised
// at the byte level. The cached state includes failures: if the
// initial open fails, every later call surfaces the same error
// without retrying.
// ---------------------------------------------------------------------------

static INPUT: OnceLock<io::Result<Mutex<File>>> = OnceLock::new();
static OUTPUT: OnceLock<io::Result<Mutex<File>>> = OnceLock::new();

fn input_lock() -> io::Result<&'static Mutex<File>> {
    cached(INPUT.get_or_init(open_input))
}

fn output_lock() -> io::Result<&'static Mutex<File>> {
    cached(OUTPUT.get_or_init(open_output))
}

fn cached(slot: &'static io::Result<Mutex<File>>) -> io::Result<&'static Mutex<File>> {
    match slot {
        Ok(m) => Ok(m),
        // `io::Error` is not `Clone`; reconstruct the cached error
        // with the same kind and message so each caller sees an
        // equivalent value.
        Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
    }
}

#[cfg(unix)]
fn open_input() -> io::Result<Mutex<File>> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map(Mutex::new)
}

#[cfg(unix)]
fn open_output() -> io::Result<Mutex<File>> {
    // The output side gets its own `File` (a `dup` of the same
    // underlying `/dev/tty`), so taking the input lock does not block
    // writes and vice versa.
    let file = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    Ok(Mutex::new(file))
}

#[cfg(windows)]
fn open_input() -> io::Result<Mutex<File>> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONIN$")
        .map(Mutex::new)
}

#[cfg(windows)]
fn open_output() -> io::Result<Mutex<File>> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
        .map(Mutex::new)
}

#[cfg(not(any(unix, windows)))]
fn open_input() -> io::Result<Mutex<File>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "open_tty is not available on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn open_output() -> io::Result<Mutex<File>> {
    open_input()
}

/// Open the controlling terminal and return separate input / output
/// handles.
///
/// On Unix both halves refer to `/dev/tty`; on Windows the input is
/// `CONIN$` and the output is `CONOUT$`. The underlying handles are
/// opened on the first successful call and cached for the lifetime of
/// the process; every later call returns a fresh `Copy` handle that
/// references the same cached state, so calling `open_tty` repeatedly
/// is cheap.
///
/// Returns an error if the process has no controlling terminal or if
/// the device cannot be opened. Failures are also cached: once a
/// platform open has failed, subsequent calls return an equivalent
/// error without retrying.
pub fn open_tty() -> io::Result<(TtyInput, TtyOutput)> {
    Ok((
        TtyInput {
            inner: input_lock()?,
        },
        TtyOutput {
            inner: output_lock()?,
        },
    ))
}

/// Read end of the controlling terminal returned by [`open_tty`].
///
/// A cheap `Copy` handle that references a process-wide cached
/// [`File`] guarded by a [`Mutex`]; concurrent reads from multiple
/// threads are serialised at the byte level.
#[derive(Clone, Copy)]
pub struct TtyInput {
    inner: &'static Mutex<File>,
}

/// Write end of the controlling terminal returned by [`open_tty`].
///
/// A cheap `Copy` handle that references a process-wide cached
/// [`File`] guarded by a [`Mutex`]; concurrent writes from multiple
/// threads are serialised at the byte level.
///
/// On Windows, [`Write::write`] detects when the underlying handle
/// refers to a console and transcodes UTF-8 to UTF-16 +
/// `WriteConsoleW` so non-ASCII text renders correctly; non-console
/// handles (files or pipes) fall through to plain `WriteFile`.
#[derive(Clone, Copy)]
pub struct TtyOutput {
    inner: &'static Mutex<File>,
}

impl fmt::Debug for TtyInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtyInput").finish()
    }
}

impl fmt::Debug for TtyOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtyOutput").finish()
    }
}

impl Read for TtyInput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }
}

impl Read for &TtyInput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (&*guard).read(buf)
    }
}

impl Write for TtyOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
}

impl Write for &TtyOutput {
    #[cfg(windows)]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        write_to_console_or_file(&guard, buf)
    }

    #[cfg(not(windows))]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (&*guard).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (&*guard).flush()
    }
}

#[cfg(unix)]
impl AsFd for TtyInput {
    fn as_fd(&self) -> BorrowedFd<'_> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the cached `File` lives for the lifetime of the
        // process and its fd never changes; the returned borrow is
        // bounded by `&self` so it cannot outlive the caller's handle.
        unsafe { BorrowedFd::borrow_raw(guard.as_raw_fd()) }
    }
}

#[cfg(unix)]
impl AsRawFd for TtyInput {
    fn as_raw_fd(&self) -> RawFd {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_raw_fd()
    }
}

#[cfg(unix)]
impl AsFd for TtyOutput {
    fn as_fd(&self) -> BorrowedFd<'_> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: see `AsFd for TtyInput`.
        unsafe { BorrowedFd::borrow_raw(guard.as_raw_fd()) }
    }
}

#[cfg(unix)]
impl AsRawFd for TtyOutput {
    fn as_raw_fd(&self) -> RawFd {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsHandle for TtyInput {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the cached `File` lives for the lifetime of the
        // process and its handle never changes; the returned borrow
        // is bounded by `&self`.
        unsafe { BorrowedHandle::borrow_raw(guard.as_raw_handle() as _) }
    }
}

#[cfg(windows)]
impl AsRawHandle for TtyInput {
    fn as_raw_handle(&self) -> RawHandle {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_raw_handle()
    }
}

#[cfg(windows)]
impl AsHandle for TtyOutput {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: see `AsHandle for TtyInput`.
        unsafe { BorrowedHandle::borrow_raw(guard.as_raw_handle() as _) }
    }
}

#[cfg(windows)]
impl AsRawHandle for TtyOutput {
    fn as_raw_handle(&self) -> RawHandle {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_raw_handle()
    }
}

#[cfg(windows)]
fn is_console(h: RawHandle) -> bool {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Console::GetConsoleMode;
    let mut mode: u32 = 0;
    unsafe { GetConsoleMode(h as HANDLE, &mut mode) != 0 }
}

#[cfg(windows)]
fn write_to_console_or_file(file: &File, buf: &[u8]) -> io::Result<usize> {
    if !is_console(file.as_raw_handle()) {
        return (&*file).write(buf);
    }

    // Console path: decode the largest UTF-8 prefix of `buf`, transcode
    // it to UTF-16, and write it with WriteConsoleW. Any trailing
    // partial codepoint stays in the caller's buffer (BufWriter will
    // include it in the next call).
    let utf8 = match std::str::from_utf8(buf) {
        Ok(s) => s,
        Err(e) => {
            let valid = e.valid_up_to();
            if valid == 0 {
                // No complete codepoint at all: emit U+FFFD and report
                // one byte consumed so the caller makes forward
                // progress. Matches std's behavior on a console.
                return write_utf16_console(file.as_raw_handle(), &['\u{FFFD}' as u16]).map(|_| 1);
            }
            // SAFETY: `valid` is the byte offset of the last valid
            // UTF-8 boundary as reported by Utf8Error::valid_up_to.
            unsafe { std::str::from_utf8_unchecked(&buf[..valid]) }
        }
    };

    let utf16: Vec<u16> = utf8.encode_utf16().collect();
    if utf16.is_empty() {
        return Ok(0);
    }
    let units = write_utf16_console(file.as_raw_handle(), &utf16)?;

    // Translate the count of UTF-16 code units back to the byte length
    // of the corresponding UTF-8 prefix.
    let mut consumed_units = 0usize;
    let mut consumed_bytes = 0usize;
    for c in utf8.chars() {
        let cu = c.len_utf16();
        if consumed_units + cu > units {
            break;
        }
        consumed_units += cu;
        consumed_bytes += c.len_utf8();
        if consumed_units == units {
            break;
        }
    }
    Ok(consumed_bytes)
}

#[cfg(windows)]
fn write_utf16_console(h: RawHandle, data: &[u16]) -> io::Result<usize> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Console::WriteConsoleW;
    let mut written: u32 = 0;
    let ok = unsafe {
        WriteConsoleW(
            h as HANDLE,
            data.as_ptr(),
            data.len() as u32,
            &mut written,
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(written as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_tty_returns_either_pair_or_error() {
        // In CI / piped contexts there may be no controlling tty; both
        // outcomes are acceptable. We're verifying that the call
        // doesn't panic and produces a usable pair when one is
        // available.
        match open_tty() {
            Ok((input, _output)) => {
                #[cfg(unix)]
                {
                    assert!(input.as_raw_fd() >= 0);
                }
                #[cfg(windows)]
                {
                    let _ = input.as_raw_handle();
                }
                #[cfg(not(any(unix, windows)))]
                {
                    let _ = input;
                }
            }
            Err(_e) => {
                // Any io::Error is acceptable — in CI / piped
                // contexts the error kind varies across platforms
                // (NotFound, PermissionDenied, Unsupported, or
                // Uncategorized ENXIO when no controlling tty is
                // attached). The contract under test is just that
                // open_tty surfaces a clean error rather than
                // panicking.
            }
        }
    }

    #[test]
    fn repeated_calls_share_underlying_handle() {
        let Ok((input_a, output_a)) = open_tty() else {
            return;
        };
        let Ok((input_b, output_b)) = open_tty() else {
            unreachable!("second open_tty must succeed when the first did");
        };

        // Successive open_tty calls return Copy handles that wrap the
        // same cached descriptors, not freshly-opened ones.
        #[cfg(unix)]
        {
            assert_eq!(input_a.as_raw_fd(), input_b.as_raw_fd());
            assert_eq!(output_a.as_raw_fd(), output_b.as_raw_fd());
        }
        #[cfg(windows)]
        {
            assert_eq!(input_a.as_raw_handle(), input_b.as_raw_handle());
            assert_eq!(output_a.as_raw_handle(), output_b.as_raw_handle());
        }
    }
}

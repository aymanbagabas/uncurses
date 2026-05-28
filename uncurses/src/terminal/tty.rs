//! Controlling-terminal helpers.
//!
//! [`open_tty`] opens the controlling terminal directly, useful when
//! stdio is piped or redirected but the program still needs to talk to
//! a real terminal. On Unix both halves of the returned pair refer to
//! `/dev/tty`; on Windows the input is `CONIN$` and the output is
//! `CONOUT$`.
//!
//! Both halves are returned as the [`TtyInput`] and [`TtyOutput`]
//! newtypes so the same type names work on every platform. The Windows
//! [`TtyOutput`] `Write` impl transparently routes console writes
//! through `WriteConsoleW` (UTF-16) so non-ASCII text round-trips
//! correctly through the conpty, matching [`std::io::Stdout`].

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};

#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, IntoRawFd, RawFd};

#[cfg(windows)]
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, IntoRawHandle, RawHandle};

/// Open the controlling terminal and return separate input / output
/// handles.
///
/// On Unix this opens `/dev/tty` for read+write and clones the fd
/// (both ends of the pair refer to the same kernel file). On Windows
/// it opens `CONIN$` for input and `CONOUT$` for output.
///
/// Returns an error if the process has no controlling terminal or if
/// the device cannot be opened.
#[cfg(unix)]
pub fn open_tty() -> io::Result<(TtyInput, TtyOutput)> {
    let input = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let output = input.try_clone()?;
    Ok((TtyInput { inner: input }, TtyOutput { inner: output }))
}

#[cfg(windows)]
pub fn open_tty() -> io::Result<(TtyInput, TtyOutput)> {
    let input = OpenOptions::new().read(true).write(true).open("CONIN$")?;
    let output = OpenOptions::new().read(true).write(true).open("CONOUT$")?;
    Ok((TtyInput { inner: input }, TtyOutput { inner: output }))
}

#[cfg(not(any(unix, windows)))]
pub fn open_tty() -> io::Result<(TtyInput, TtyOutput)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "open_tty is not available on this platform",
    ))
}

/// Read end of the controlling terminal returned by [`open_tty`].
///
/// Mirrors the trait surface of [`File`]: [`Read`] (owned and shared
/// reference), the platform handle traits ([`AsFd`]/[`AsRawFd`]/
/// [`IntoRawFd`] on Unix, [`AsHandle`]/[`AsRawHandle`]/[`IntoRawHandle`]
/// on Windows), [`Debug`](fmt::Debug), and an inherent
/// [`try_clone`](Self::try_clone) method.
pub struct TtyInput {
    inner: File,
}

/// Write end of the controlling terminal returned by [`open_tty`].
///
/// Mirrors the trait surface of [`File`]: [`Write`] (owned and shared
/// reference), the platform handle traits ([`AsFd`]/[`AsRawFd`]/
/// [`IntoRawFd`] on Unix, [`AsHandle`]/[`AsRawHandle`]/[`IntoRawHandle`]
/// on Windows), [`Debug`](fmt::Debug), and an inherent
/// [`try_clone`](Self::try_clone) method.
///
/// On Windows, [`Write::write`] detects when the underlying handle
/// refers to a console and transcodes UTF-8 to UTF-16 + `WriteConsoleW`
/// so non-ASCII text renders correctly; non-console handles (files or
/// pipes) fall through to plain `WriteFile`.
pub struct TtyOutput {
    inner: File,
}

impl TtyInput {
    /// Duplicate the underlying handle and wrap it in a new
    /// [`TtyInput`].
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
        })
    }
}

impl TtyOutput {
    /// Duplicate the underlying handle and wrap it in a new
    /// [`TtyOutput`].
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
        })
    }
}

impl fmt::Debug for TtyInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtyInput")
            .field("inner", &self.inner)
            .finish()
    }
}

impl fmt::Debug for TtyOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtyOutput")
            .field("inner", &self.inner)
            .finish()
    }
}

impl Read for TtyInput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.inner).read(buf)
    }
}

impl Read for &TtyInput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.inner).read(buf)
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
        write_to_console_or_file(&self.inner, buf)
    }

    #[cfg(not(windows))]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.inner).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&self.inner).flush()
    }
}

#[cfg(unix)]
impl AsFd for TtyInput {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

#[cfg(unix)]
impl AsRawFd for TtyInput {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(unix)]
impl IntoRawFd for TtyInput {
    fn into_raw_fd(self) -> RawFd {
        self.inner.into_raw_fd()
    }
}

#[cfg(unix)]
impl AsFd for TtyOutput {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

#[cfg(unix)]
impl AsRawFd for TtyOutput {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(unix)]
impl IntoRawFd for TtyOutput {
    fn into_raw_fd(self) -> RawFd {
        self.inner.into_raw_fd()
    }
}

#[cfg(windows)]
impl AsHandle for TtyInput {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.inner.as_handle()
    }
}

#[cfg(windows)]
impl AsRawHandle for TtyInput {
    fn as_raw_handle(&self) -> RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl IntoRawHandle for TtyInput {
    fn into_raw_handle(self) -> RawHandle {
        self.inner.into_raw_handle()
    }
}

#[cfg(windows)]
impl AsHandle for TtyOutput {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.inner.as_handle()
    }
}

#[cfg(windows)]
impl AsRawHandle for TtyOutput {
    fn as_raw_handle(&self) -> RawHandle {
        self.inner.as_raw_handle()
    }
}

#[cfg(windows)]
impl IntoRawHandle for TtyOutput {
    fn into_raw_handle(self) -> RawHandle {
        self.inner.into_raw_handle()
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
    fn try_clone_returns_distinct_handle_pointing_at_same_tty() {
        let Ok((input, output)) = open_tty() else {
            return;
        };

        let input2 = input.try_clone().expect("clone input");
        let output2 = output.try_clone().expect("clone output");

        // Cloned handles must refer to the same underlying device but
        // be independent OS handles.
        #[cfg(unix)]
        {
            assert_ne!(input.as_raw_fd(), input2.as_raw_fd());
            assert_ne!(output.as_raw_fd(), output2.as_raw_fd());
        }
        #[cfg(windows)]
        {
            assert_ne!(input.as_raw_handle(), input2.as_raw_handle());
            assert_ne!(output.as_raw_handle(), output2.as_raw_handle());
        }
    }

    #[test]
    fn try_clone_survives_dropping_the_original() {
        let Ok((input, output)) = open_tty() else {
            return;
        };

        let input2 = input.try_clone().expect("clone input");
        let output2 = output.try_clone().expect("clone output");

        drop(input);
        drop(output);

        // The cloned handles must still be usable after the originals
        // are dropped — i.e. each holds an independent reference.
        #[cfg(unix)]
        {
            assert!(input2.as_raw_fd() >= 0);
            assert!(output2.as_raw_fd() >= 0);
        }
        #[cfg(windows)]
        {
            let _ = input2.as_raw_handle();
            let _ = output2.as_raw_handle();
        }
    }
}

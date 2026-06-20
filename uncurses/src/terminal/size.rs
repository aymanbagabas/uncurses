//! Terminal window-size detection.
//!
//! [`get_window_size`] queries the operating system for the visible terminal
//! dimensions. Sizes are reported as [`Winsize`]: rows and columns in terminal
//! cells, plus pixel dimensions when the platform exposes them.

use std::io;

#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd};
#[cfg(windows)]
use std::os::windows::io::{AsHandle, AsRawHandle};
#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;

/// Terminal dimensions in cells and pixels.
///
/// `row` and `col` are the cell dimensions used for terminal layout. `xpixel`
/// and `ypixel` are optional pixel dimensions; they are `0` when the platform
/// or terminal does not report pixel size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Winsize {
    /// Number of rows (height in cells).
    pub row: u16,
    /// Number of columns (width in cells).
    pub col: u16,
    /// Width in pixels, or `0` when unknown.
    pub xpixel: u16,
    /// Height in pixels, or `0` when unknown.
    pub ypixel: u16,
}

impl Default for Winsize {
    /// Return the conventional fallback size of 80 columns by 24 rows.
    ///
    /// Pixel dimensions are unknown and set to `0`.
    fn default() -> Self {
        Self {
            row: 24,
            col: 80,
            xpixel: 0,
            ypixel: 0,
        }
    }
}

impl From<Winsize> for (u16, u16) {
    /// Convert to a `(width, height)` cell pair.
    ///
    /// The returned tuple is `(col, row)`. Pixel fields are dropped.
    fn from(ws: Winsize) -> Self {
        (ws.col, ws.row)
    }
}

/// Query the terminal size attached to `fd`.
///
/// This calls `TIOCGWINSZ` and returns the kernel-provided row, column, and
/// pixel fields.
///
/// # Parameters
///
/// * `fd` — descriptor to query.
///
/// # Returns
///
/// The current [`Winsize`].
///
/// # Errors
///
/// Returns the OS error if `ioctl` fails, for example because `fd` is not a
/// terminal.
///
/// # Panics
///
/// This function does not intentionally panic.
#[cfg(unix)]
pub fn get_window_size<F: AsFd>(fd: F) -> io::Result<Winsize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::ioctl(fd.as_fd().as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Winsize {
        row: ws.ws_row,
        col: ws.ws_col,
        xpixel: ws.ws_xpixel,
        ypixel: ws.ws_ypixel,
    })
}

/// Query the visible console window size attached to `h`.
///
/// Pixel dimensions are unavailable on this platform and are returned as `0`.
///
/// # Parameters
///
/// * `h` — console screen-buffer handle to query.
///
/// # Returns
///
/// The current [`Winsize`] in cells.
///
/// # Errors
///
/// Returns the OS error if `GetConsoleScreenBufferInfo` fails, for example
/// because `h` is not a console output handle.
///
/// # Panics
///
/// This function does not intentionally panic.
#[cfg(windows)]
pub fn get_window_size<H: AsHandle>(h: H) -> io::Result<Winsize> {
    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo,
    };

    let handle = h.as_handle().as_raw_handle() as HANDLE;
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    if unsafe { GetConsoleScreenBufferInfo(handle, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let col = (info.srWindow.Right - info.srWindow.Left + 1).max(0) as u16;
    let row = (info.srWindow.Bottom - info.srWindow.Top + 1).max(0) as u16;
    Ok(Winsize {
        row,
        col,
        xpixel: 0,
        ypixel: 0,
    })
}

#[cfg(not(any(unix, windows)))]
/// Query the terminal size on an unsupported platform.
///
/// Always returns [`io::ErrorKind::Unsupported`].
pub fn get_window_size<T>(_: T) -> io::Result<Winsize> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "get_window_size is not implemented for this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_size_is_80x24() {
        let s = Winsize::default();
        assert_eq!(s.col, 80);
        assert_eq!(s.row, 24);
    }
}

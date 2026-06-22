//! Raw mode and terminal state helpers.
//!
//! These free functions operate on terminal descriptors on Unix and console
//! handles on Windows. [`make_raw_mode`] saves the current configuration,
//! applies raw-mode settings immediately, and returns the previous [`State`].
//! Pass that state to [`set_state`] with the same handles to restore it.
//!
//! ```text
//! get_state() ── snapshot only ───────────────────────────────┐
//!                                                             │
//! make_raw_mode() ── returns previous State ── raw mode ── set_state()
//! ```
//!
//! [`Terminal::make_raw`](super::Terminal::make_raw) and
//! [`Terminal::restore`](super::Terminal::restore) wrap this same flow and keep
//! one saved state inside the terminal handle.

use std::io;

#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd};
#[cfg(windows)]
use std::os::windows::io::{AsHandle, AsRawHandle};
#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;

/// Snapshot of a terminal's configuration.
///
/// On Unix this stores a `libc::termios` value read from the input descriptor,
/// falling back to the output descriptor if necessary. On Windows it stores
/// both input and output console-mode bitfields.
///
/// Use values returned by [`get_state`] or [`make_raw_mode`] with
/// [`set_state`] to restore a terminal to a previous configuration.
#[derive(Clone)]
pub struct State {
    #[cfg(unix)]
    /// Saved terminal attributes.
    pub termios: libc::termios,
    #[cfg(windows)]
    /// Saved input console-mode bits.
    pub input_mode: u32,
    #[cfg(windows)]
    /// Saved output console-mode bits.
    pub output_mode: u32,
}

#[cfg(windows)]
unsafe impl Send for State {}
#[cfg(windows)]
unsafe impl Sync for State {}

/// Read the current terminal state.
///
/// On Unix the input descriptor is tried first and the output descriptor is
/// the fallback. On Windows both input and output console modes are sampled.
///
/// # Parameters
///
/// * `input` — terminal input descriptor or handle.
/// * `output` — terminal output descriptor or handle.
///
/// # Returns
///
/// A [`State`] describing the current terminal mode.
///
/// # Errors
///
/// Returns the OS error from the state query. On Unix, an error is returned
/// only if both input and output descriptors fail.
///
/// # Panics
///
/// This function does not intentionally panic.
#[cfg(unix)]
pub fn get_state<I: AsFd, O: AsFd>(input: I, output: O) -> io::Result<State> {
    use std::mem::MaybeUninit;

    fn try_read(fd: i32) -> io::Result<libc::termios> {
        let mut t = MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(fd, t.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { t.assume_init() })
    }

    let ifd = input.as_fd().as_raw_fd();
    let ofd = output.as_fd().as_raw_fd();
    let termios = try_read(ifd).or_else(|_| try_read(ofd))?;
    Ok(State { termios })
}

/// Apply `state` to the terminal immediately.
///
/// On Unix this uses `TCSANOW`; the input descriptor is tried first and the
/// output descriptor is the fallback. On Windows both modes stored in
/// [`State`] are written.
///
/// # Parameters
///
/// * `input` — terminal input descriptor or handle.
/// * `output` — terminal output descriptor or handle.
/// * `state` — state previously returned by [`get_state`] or
///   [`make_raw_mode`].
///
/// # Returns
///
/// `Ok(())` when the state was applied.
///
/// # Errors
///
/// Returns the OS error from applying the state. On Unix, an error is returned
/// only if applying to both input and output descriptors fails.
///
/// # Panics
///
/// This function does not intentionally panic.
#[cfg(unix)]
pub fn set_state<I: AsFd, O: AsFd>(input: I, output: O, state: &State) -> io::Result<()> {
    let ifd = input.as_fd().as_raw_fd();
    let ofd = output.as_fd().as_raw_fd();
    if unsafe { libc::tcsetattr(ifd, libc::TCSANOW, &state.termios) } == 0 {
        return Ok(());
    }
    if unsafe { libc::tcsetattr(ofd, libc::TCSANOW, &state.termios) } == 0 {
        return Ok(());
    }
    Err(io::Error::last_os_error())
}

#[cfg(windows)]
/// Read the current console modes.
///
/// Both input and output handles must support `GetConsoleMode`.
///
/// # Parameters
///
/// * `input` — console input handle.
/// * `output` — console output handle.
///
/// # Returns
///
/// A [`State`] containing both console-mode bitfields.
///
/// # Errors
///
/// Returns the OS error if either console mode cannot be read.
///
/// # Panics
///
/// This function does not intentionally panic.
pub fn get_state<I: AsHandle, O: AsHandle>(input: I, output: O) -> io::Result<State> {
    use windows_sys::Win32::System::Console::GetConsoleMode;

    let ih = input.as_handle().as_raw_handle() as HANDLE;
    let oh = output.as_handle().as_raw_handle() as HANDLE;

    let mut input_mode: u32 = 0;
    if unsafe { GetConsoleMode(ih, &mut input_mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut output_mode: u32 = 0;
    if unsafe { GetConsoleMode(oh, &mut output_mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(State {
        input_mode,
        output_mode,
    })
}

#[cfg(windows)]
/// Apply console modes from `state`.
///
/// # Parameters
///
/// * `input` — console input handle.
/// * `output` — console output handle.
/// * `state` — console modes to apply.
///
/// # Returns
///
/// `Ok(())` when both input and output modes were applied.
///
/// # Errors
///
/// Returns the OS error if either `SetConsoleMode` call fails. If the output
/// mode fails after the input mode succeeds, the input mode is not rolled back.
///
/// # Panics
///
/// This function does not intentionally panic.
pub fn set_state<I: AsHandle, O: AsHandle>(input: I, output: O, state: &State) -> io::Result<()> {
    use windows_sys::Win32::System::Console::SetConsoleMode;

    let ih = input.as_handle().as_raw_handle() as HANDLE;
    let oh = output.as_handle().as_raw_handle() as HANDLE;

    if unsafe { SetConsoleMode(ih, state.input_mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { SetConsoleMode(oh, state.output_mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Place the terminal into raw mode.
///
/// On Unix this applies a `cfmakeraw(3)`-equivalent termios (`VMIN = 1`,
/// `VTIME = 0`) using [`set_state`]. The flags match glibc's `cfmakeraw` on
/// every platform, so raw mode behaves identically everywhere rather than
/// following each libc's own variation. On Windows the input handle has cooked
/// input flags and quick-edit cleared and virtual-terminal/window-input flags
/// set; the output handle has processed output, virtual-terminal processing,
/// and newline-auto-return disabling set.
///
/// # Parameters
///
/// * `input` — terminal input descriptor or handle.
/// * `output` — terminal output descriptor or handle.
///
/// # Returns
///
/// The pre-call [`State`]. Pass it to [`set_state`] to restore.
///
/// # Errors
///
/// Returns any error from reading the current state or applying the raw state.
///
/// # Panics
///
/// This function does not intentionally panic.
#[cfg(unix)]
pub fn make_raw_mode<I: AsFd, O: AsFd>(input: I, output: O) -> io::Result<State> {
    let original = get_state(&input, &output)?;
    let mut raw = original.termios;
    raw.c_iflag &= !(libc::IGNBRK
        | libc::BRKINT
        | libc::PARMRK
        | libc::ISTRIP
        | libc::INLCR
        | libc::IGNCR
        | libc::ICRNL
        | libc::IXON);
    raw.c_oflag &= !libc::OPOST;
    raw.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
    raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
    raw.c_cflag |= libc::CS8;
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;

    set_state(&input, &output, &State { termios: raw })?;
    Ok(original)
}

#[cfg(windows)]
/// Place the console into raw mode.
///
/// The input handle has cooked input flags cleared and virtual-terminal/window
/// input flags set; the output handle has virtual-terminal processing and
/// newline-auto-return disabling set.
///
/// # Parameters
///
/// * `input` — console input handle.
/// * `output` — console output handle.
///
/// # Returns
///
/// The pre-call [`State`]. Pass it to [`set_state`] to restore.
///
/// # Errors
///
/// Returns any error from reading the current modes or applying the raw modes.
///
/// # Panics
///
/// This function does not intentionally panic.
pub fn make_raw_mode<I: AsHandle, O: AsHandle>(input: I, output: O) -> io::Result<State> {
    use windows_sys::Win32::System::Console::{
        DISABLE_NEWLINE_AUTO_RETURN, ENABLE_ECHO_INPUT, ENABLE_EXTENDED_FLAGS, ENABLE_LINE_INPUT,
        ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT, ENABLE_QUICK_EDIT_MODE,
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, ENABLE_WINDOW_INPUT,
    };

    let original = get_state(&input, &output)?;
    // Clearing quick-edit while setting extended-flags is how the console API
    // disables mouse selection, which would otherwise swallow mouse input.
    let raw_input = (original.input_mode
        & !(ENABLE_ECHO_INPUT
            | ENABLE_PROCESSED_INPUT
            | ENABLE_LINE_INPUT
            | ENABLE_QUICK_EDIT_MODE))
        | ENABLE_VIRTUAL_TERMINAL_INPUT
        | ENABLE_EXTENDED_FLAGS
        | ENABLE_WINDOW_INPUT;
    // Virtual-terminal processing requires processed output, so set it too.
    let raw_output = original.output_mode
        | ENABLE_PROCESSED_OUTPUT
        | ENABLE_VIRTUAL_TERMINAL_PROCESSING
        | DISABLE_NEWLINE_AUTO_RETURN;

    set_state(
        &input,
        &output,
        &State {
            input_mode: raw_input,
            output_mode: raw_output,
        },
    )?;
    Ok(original)
}

/// Return whether the descriptor is connected to a terminal.
///
/// # Parameters
///
/// * `fd` — descriptor to test.
///
/// # Returns
///
/// `true` when `fd` refers to a terminal.
///
/// # Errors and panics
///
/// This function does not fail or intentionally panic.
#[cfg(unix)]
pub fn is_terminal<F: AsFd>(fd: F) -> bool {
    unsafe { libc::isatty(fd.as_fd().as_raw_fd()) != 0 }
}

#[cfg(windows)]
/// Return whether the handle is connected to a console.
///
/// # Parameters
///
/// * `h` — handle to test.
///
/// # Returns
///
/// `true` when `h` supports `GetConsoleMode`.
///
/// # Errors and panics
///
/// This function does not fail or intentionally panic.
pub fn is_terminal<H: AsHandle>(h: H) -> bool {
    use windows_sys::Win32::System::Console::GetConsoleMode;
    let handle = h.as_handle().as_raw_handle() as HANDLE;
    let mut mode: u32 = 0;
    unsafe { GetConsoleMode(handle, &mut mode) != 0 }
}

#[cfg(not(any(unix, windows)))]
/// Return whether a handle is connected to a terminal.
///
/// On unsupported platforms this always returns `false`.
pub fn is_terminal<T>(_: T) -> bool {
    false
}

//! Raw mode and terminal state helpers.
//!
//! Free functions that operate directly on a file descriptor (Unix) or
//! a console handle (Windows). [`make_raw_mode`] returns a [`State`]
//! capturing the previous configuration; pass it back to [`set_state`]
//! (along with the same handles) to restore.
//!
//! [`get_state`] / [`set_state`] expose the same snapshot type for
//! arbitrary save/restore use outside of raw mode.

use std::io;

#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd};
#[cfg(windows)]
use std::os::windows::io::{AsHandle, AsRawHandle};
#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;

/// Snapshot of a terminal's configuration.
///
/// On Unix this is the `libc::termios` for the input side. On Windows
/// it is the input and output console-mode bits.
#[derive(Clone)]
pub struct State {
    #[cfg(unix)]
    /// Saved terminal attributes.
    pub termios: libc::termios,
    #[cfg(windows)]
    pub input_mode: u32,
    #[cfg(windows)]
    pub output_mode: u32,
}

#[cfg(windows)]
unsafe impl Send for State {}
#[cfg(windows)]
unsafe impl Sync for State {}

/// Read the current terminal state.
///
/// On Unix the input fd is tried first and the output fd is the
/// fallback. On Windows both modes are sampled.
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

/// Apply `state` to the terminal (`TCSANOW` on Unix).
///
/// On Unix the input fd is tried first and the output fd is the
/// fallback. On Windows both modes are written.
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

/// Place the terminal into "raw" mode.
///
/// On Unix this applies a `cfmakeraw(3)`-equivalent termios
/// (`VMIN = 1`, `VTIME = 0`). On Windows the input handle has the
/// cooked flags cleared and `ENABLE_VIRTUAL_TERMINAL_INPUT |
/// ENABLE_EXTENDED_FLAGS | ENABLE_WINDOW_INPUT` ORed in; the output
/// handle has `ENABLE_VIRTUAL_TERMINAL_PROCESSING |
/// DISABLE_NEWLINE_AUTO_RETURN` ORed in.
///
/// Returns the pre-call [`State`]; pass it to [`set_state`] to restore.
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
pub fn make_raw_mode<I: AsHandle, O: AsHandle>(input: I, output: O) -> io::Result<State> {
    use windows_sys::Win32::System::Console::{
        DISABLE_NEWLINE_AUTO_RETURN, ENABLE_ECHO_INPUT, ENABLE_EXTENDED_FLAGS, ENABLE_LINE_INPUT,
        ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        ENABLE_WINDOW_INPUT,
    };

    let original = get_state(&input, &output)?;
    let raw_input = (original.input_mode
        & !(ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT))
        | ENABLE_VIRTUAL_TERMINAL_INPUT
        | ENABLE_EXTENDED_FLAGS
        | ENABLE_WINDOW_INPUT;
    let raw_output =
        original.output_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN;

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

/// Is the descriptor connected to a terminal?
#[cfg(unix)]
pub fn is_terminal<F: AsFd>(fd: F) -> bool {
    unsafe { libc::isatty(fd.as_fd().as_raw_fd()) != 0 }
}

#[cfg(windows)]
pub fn is_terminal<H: AsHandle>(h: H) -> bool {
    use windows_sys::Win32::System::Console::GetConsoleMode;
    let handle = h.as_handle().as_raw_handle() as HANDLE;
    let mut mode: u32 = 0;
    unsafe { GetConsoleMode(handle, &mut mode) != 0 }
}

#[cfg(not(any(unix, windows)))]
pub fn is_terminal<T>(_: T) -> bool {
    false
}

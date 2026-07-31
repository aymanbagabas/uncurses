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
//! The input and output halves are tracked separately. Nothing requires them
//! to be the same device: [`Terminal`](super::Terminal) is generic over its
//! two descriptors, so a caller can pair two unrelated terminals, or pair a
//! terminal with a pipe. Each half is read, rawified, and restored from its
//! own attributes, and a half that is not a terminal is skipped.
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
/// On Unix this stores one `libc::termios` per half, because the input and
/// output descriptors are not required to refer to the same terminal device. A
/// half whose attributes cannot be read is stored as `None` and is skipped when
/// the state is applied. On Windows it stores both input and output
/// console-mode bitfields.
///
/// A `State` records each half separately, so it is bound to the pair it was
/// read from. Apply it to that same `(input, output)` pair; applying it to a
/// different pair writes each half's attributes to a descriptor they did not
/// come from.
///
/// Use values returned by [`Terminal::get_state`](crate::terminal::Terminal::get_state)
/// or [`Terminal::make_raw`](crate::terminal::Terminal::make_raw) with
/// [`Terminal::set_state`](crate::terminal::Terminal::set_state) to restore a
/// terminal to a previous configuration.
#[derive(Clone)]
pub struct State {
    #[cfg(unix)]
    /// Saved attributes of the input descriptor, or `None` when they could not
    /// be read — typically because it is not a terminal.
    pub(crate) input: Option<libc::termios>,
    #[cfg(unix)]
    /// Saved attributes of the output descriptor, or `None` when they could not
    /// be read — typically because it is not a terminal.
    pub(crate) output: Option<libc::termios>,
    #[cfg(windows)]
    /// Saved input console-mode bits.
    pub(crate) input_mode: u32,
    #[cfg(windows)]
    /// Saved output console-mode bits.
    pub(crate) output_mode: u32,
}

#[cfg(windows)]
unsafe impl Send for State {}
#[cfg(windows)]
unsafe impl Sync for State {}

/// Read the current terminal state.
///
/// On Unix each descriptor is read independently, so a pair pointing at two
/// different terminal devices is described in full and a half that is not a
/// terminal is recorded as `None`. On Windows both input and output console
/// modes are sampled.
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
/// only if the attributes of neither descriptor can be read — typically
/// because neither is a terminal — and the output descriptor's error is the
/// one reported.
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

    let input = try_read(input.as_fd().as_raw_fd()).ok();
    let output = try_read(output.as_fd().as_raw_fd());
    match (input, output) {
        // Neither half is a terminal. Report the output error, which is what
        // this function surfaced when it fell back from input to output.
        (None, Err(e)) => Err(e),
        (input, output) => Ok(State {
            input,
            output: output.ok(),
        }),
    }
}

/// Apply `state` to the terminal immediately.
///
/// On Unix this uses `TCSANOW` and writes each half of `state` to its own
/// descriptor; a half recorded as `None` is skipped. Both halves are attempted
/// even if the first one fails, so a partial failure still restores as much as
/// it can. On Windows both modes stored in [`State`] are written.
///
/// When the two descriptors refer to the same terminal, this applies identical
/// attributes to that device twice. `tcsetattr` is idempotent, so the second
/// call is a no-op.
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
/// `Ok(())` when every half recorded in `state` was applied.
///
/// # Errors
///
/// Returns the OS error from applying the state; the input descriptor's error
/// is the one reported when both halves fail. A half recorded as `None` is not
/// an error. If one half fails after the other succeeded, the successful half
/// is not rolled back.
///
/// # Panics
///
/// This function does not intentionally panic.
#[cfg(unix)]
pub fn set_state<I: AsFd, O: AsFd>(input: I, output: O, state: &State) -> io::Result<()> {
    fn try_write(fd: i32, termios: Option<&libc::termios>) -> io::Result<()> {
        let Some(termios) = termios else {
            return Ok(());
        };
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, termios) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    let applied_input = try_write(input.as_fd().as_raw_fd(), state.input.as_ref());
    let applied_output = try_write(output.as_fd().as_raw_fd(), state.output.as_ref());
    applied_input.and(applied_output)
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
/// Returns the OS error if either `SetConsoleMode` call fails; the input
/// handle's error is the one reported when both fail. Both handles are
/// attempted even if the first one fails, so a partial failure still restores
/// as much as it can. If one handle fails after the other succeeded, the
/// successful one is not rolled back.
///
/// # Panics
///
/// This function does not intentionally panic.
pub fn set_state<I: AsHandle, O: AsHandle>(input: I, output: O, state: &State) -> io::Result<()> {
    use windows_sys::Win32::System::Console::SetConsoleMode;

    fn try_write(handle: HANDLE, mode: u32) -> io::Result<()> {
        if unsafe { SetConsoleMode(handle, mode) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    let ih = input.as_handle().as_raw_handle() as HANDLE;
    let oh = output.as_handle().as_raw_handle() as HANDLE;

    let applied_input = try_write(ih, state.input_mode);
    let applied_output = try_write(oh, state.output_mode);
    applied_input.and(applied_output)
}

/// Place the terminal into raw mode.
///
/// On Unix this applies a `cfmakeraw(3)`-equivalent termios (`VMIN = 1`,
/// `VTIME = 0`) using [`set_state`]. The flags match glibc's `cfmakeraw` on
/// every platform, so raw mode behaves identically everywhere rather than
/// following each libc's own variation. Each descriptor is rawified from its
/// own saved attributes, so a pair pointing at two different terminal devices
/// leaves both in raw mode. On Windows the input handle has cooked input flags
/// and quick-edit cleared and virtual-terminal/window-input flags set; the
/// output handle has processed output, virtual-terminal processing, and
/// newline-auto-return disabling set.
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
/// When only one half of a split pair accepts the raw state, the pre-call state
/// is written back to both halves before the error is returned, so a failure
/// does not leave a descriptor raw with no way to restore it. That write-back
/// is best-effort: if it fails too, the original error is still what surfaces.
///
/// # Panics
///
/// This function does not intentionally panic.
#[cfg(unix)]
pub fn make_raw_mode<I: AsFd, O: AsFd>(input: I, output: O) -> io::Result<State> {
    fn rawify(mut t: libc::termios) -> libc::termios {
        t.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::PARMRK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON);
        t.c_oflag &= !libc::OPOST;
        t.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
        t.c_cflag &= !(libc::CSIZE | libc::PARENB);
        t.c_cflag |= libc::CS8;
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
        t
    }

    let original = get_state(&input, &output)?;
    let raw = State {
        input: original.input.map(rawify),
        output: original.output.map(rawify),
    };
    if let Err(e) = set_state(&input, &output, &raw) {
        // One half may have been rawified before the other failed, and the
        // caller drops `original` along with the error. Put the pre-call state
        // back so no descriptor is stranded in raw mode; the half that failed
        // is simply rewritten its own unchanged attributes.
        let _ = set_state(&input, &output, &original);
        return Err(e);
    }
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
/// When only one handle accepts the raw modes, the pre-call modes are written
/// back to both handles before the error is returned, so a failure does not
/// leave a handle raw with no way to restore it. That write-back is
/// best-effort: if it fails too, the original error is still what surfaces.
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

    let raw = State {
        input_mode: raw_input,
        output_mode: raw_output,
    };
    if let Err(e) = set_state(&input, &output, &raw) {
        // One handle may have been switched to raw mode before the other
        // failed, and the caller drops `original` along with the error. Put the
        // pre-call modes back so no handle is stranded in raw mode; the handle
        // that failed is simply rewritten its own unchanged mode.
        let _ = set_state(&input, &output, &original);
        return Err(e);
    }
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

#[cfg(all(test, unix, not(target_os = "l4re")))]
mod tests {
    use super::*;
    use crate::testutil::{ScriptedFd, open_pty_pair};
    use std::fs::File;

    /// Every flag group `rawify` modifies. Restoration is checked against these
    /// alone: the tty driver owns the rest, and legitimately changes bits of its
    /// own — re-enabling canonical mode sets `PENDIN`, for instance — which is
    /// not something a restored state can or should undo.
    const RAW_IFLAGS: libc::tcflag_t = libc::IGNBRK
        | libc::BRKINT
        | libc::PARMRK
        | libc::ISTRIP
        | libc::INLCR
        | libc::IGNCR
        | libc::ICRNL
        | libc::IXON;
    const RAW_LFLAGS: libc::tcflag_t =
        libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN;
    const RAW_CFLAGS: libc::tcflag_t = libc::CSIZE | libc::PARENB;

    /// Read a descriptor's attributes without going through [`get_state`], so
    /// the assertions observe the device rather than the code under test.
    fn attrs<F: AsFd>(f: &F) -> libc::termios {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(f.as_fd().as_raw_fd(), &mut t) },
            0,
            "tcgetattr failed: {}",
            io::Error::last_os_error()
        );
        t
    }

    fn write_attrs(f: &File, t: &libc::termios) {
        assert_eq!(
            unsafe { libc::tcsetattr(f.as_fd().as_raw_fd(), libc::TCSANOW, t) },
            0,
            "tcsetattr failed: {}",
            io::Error::last_os_error()
        );
    }

    /// Force `OPOST` to a known value so the assertions do not depend on
    /// whatever a fresh pty happens to default to, and stamp `VMIN`/`VTIME`
    /// with values raw mode does not use. A pty defaults to `VMIN` 1, which is
    /// also raw mode's value, so restoring it would otherwise be unobservable.
    fn prime(f: &File, opost: bool) {
        let mut t = attrs(f);
        if opost {
            t.c_oflag |= libc::OPOST;
        } else {
            t.c_oflag &= !libc::OPOST;
        }
        t.c_cc[libc::VMIN] = 4;
        t.c_cc[libc::VTIME] = 7;
        write_attrs(f, &t);
    }

    fn opost(f: &File) -> bool {
        attrs(f).c_oflag & libc::OPOST != 0
    }

    /// Raw mode is more than `OPOST`, so check every flag group `rawify`
    /// touches. A single sentinel bit would let most of it regress unnoticed.
    fn assert_raw(f: &File, half: &str) {
        let t = attrs(f);
        assert_eq!(t.c_oflag & libc::OPOST, 0, "{half}: OPOST still set");
        assert_eq!(
            t.c_iflag & RAW_IFLAGS,
            0,
            "{half}: cooked input flags still set"
        );
        assert_eq!(
            t.c_lflag & RAW_LFLAGS,
            0,
            "{half}: line-editing flags still set"
        );
        assert_eq!(
            t.c_cflag & RAW_CFLAGS,
            libc::CS8,
            "{half}: not 8-bit, no parity"
        );
        assert_eq!(t.c_cc[libc::VMIN], 1, "{half}: VMIN");
        assert_eq!(t.c_cc[libc::VTIME], 0, "{half}: VTIME");
    }

    fn assert_restored(before: &libc::termios, f: &File, half: &str) {
        let now = attrs(f);
        assert_eq!(
            now.c_iflag & RAW_IFLAGS,
            before.c_iflag & RAW_IFLAGS,
            "{half}: input flags not restored"
        );
        assert_eq!(
            now.c_oflag & libc::OPOST,
            before.c_oflag & libc::OPOST,
            "{half}: OPOST not restored"
        );
        assert_eq!(
            now.c_lflag & RAW_LFLAGS,
            before.c_lflag & RAW_LFLAGS,
            "{half}: line-editing flags not restored"
        );
        assert_eq!(
            now.c_cflag & RAW_CFLAGS,
            before.c_cflag & RAW_CFLAGS,
            "{half}: control flags not restored"
        );
        assert_eq!(
            now.c_cc[libc::VMIN],
            before.c_cc[libc::VMIN],
            "{half}: VMIN not restored"
        );
        assert_eq!(
            now.c_cc[libc::VTIME],
            before.c_cc[libc::VTIME],
            "{half}: VTIME not restored"
        );
    }

    /// A pipe read end is a descriptor that is definitely not a terminal, and
    /// unlike `/dev/null` it needs nothing from the filesystem. The write end is
    /// dropped: nothing is ever read, and `tcgetattr`/`tcsetattr` reject it
    /// either way.
    fn not_a_terminal() -> std::io::PipeReader {
        std::io::pipe().expect("pipe").0
    }

    /// The two halves are independent devices, so their attributes must be
    /// sampled independently rather than one standing in for the other.
    #[test]
    fn get_state_reads_each_half_independently() {
        let (Some((_ma, a)), Some((_mb, b))) = (open_pty_pair(), open_pty_pair()) else {
            return;
        };
        prime(&a, true);
        prime(&b, false);

        let state = get_state(&a, &b).expect("both halves are terminals");
        assert!(
            state.input.expect("input half read").c_oflag & libc::OPOST != 0,
            "input half must report the input device's flags"
        );
        assert!(
            state.output.expect("output half read").c_oflag & libc::OPOST == 0,
            "output half must report the output device's flags, not the input's"
        );
    }

    /// Raw mode has to reach the device frames are written to. When the halves
    /// are two different terminals, configuring only the input one leaves
    /// `OPOST` set on the output, and the terminal keeps post-processing
    /// everything the renderer emits.
    #[test]
    fn make_raw_and_restore_cover_both_terminals() {
        let (Some((_ma, a)), Some((_mb, b))) = (open_pty_pair(), open_pty_pair()) else {
            return;
        };
        prime(&a, true);
        prime(&b, true);
        let (before_a, before_b) = (attrs(&a), attrs(&b));

        let original = make_raw_mode(&a, &b).expect("raw mode");
        assert_raw(&a, "input half");
        assert_raw(&b, "output half");

        set_state(&a, &b, &original).expect("restore");
        assert_restored(&before_a, &a, "input half");
        assert_restored(&before_b, &b, "output half");
    }

    /// Pairing a terminal with a pipe is normal, and the terminal half must
    /// still be configured whichever side it is on.
    #[test]
    fn non_terminal_half_is_skipped() {
        let Some((_master, tty)) = open_pty_pair() else {
            return;
        };
        prime(&tty, true);
        let before = attrs(&tty);

        let state = make_raw_mode(not_a_terminal(), &tty).expect("output half is a terminal");
        assert!(state.input.is_none(), "a pipe is not a terminal");
        assert!(state.output.is_some(), "the pty half must be recorded");
        assert_raw(&tty, "output half");
        set_state(not_a_terminal(), &tty, &state).expect("restore");
        assert_restored(&before, &tty, "output half");

        // ...and the same with the halves swapped.
        let state = make_raw_mode(&tty, not_a_terminal()).expect("input half is a terminal");
        assert!(state.input.is_some(), "the pty half must be recorded");
        assert!(state.output.is_none(), "a pipe is not a terminal");
        assert_raw(&tty, "input half");
        set_state(&tty, not_a_terminal(), &state).expect("restore");
        assert_restored(&before, &tty, "input half");
    }

    /// A half that was recorded as a terminal but rejects the write is a real
    /// failure and must be reported — while the other half is still attempted,
    /// so one bad descriptor cannot stop the other from being configured.
    #[test]
    fn a_half_that_fails_is_reported_and_the_other_is_still_applied() {
        let Some((_master, tty)) = open_pty_pair() else {
            return;
        };
        prime(&tty, true);

        // Reading the same pty for both halves gives a state whose halves are
        // both `Some`, so both are attempted below even though only one of the
        // descriptors can accept them.
        let mut target = get_state(&tty, &tty).expect("pty is a terminal");
        let clear_opost = |mut t: libc::termios| {
            t.c_oflag &= !libc::OPOST;
            t
        };
        target.input = target.input.map(clear_opost);
        target.output = target.output.map(clear_opost);

        assert!(
            set_state(not_a_terminal(), &tty, &target).is_err(),
            "a failing input half must be reported"
        );
        assert!(
            !opost(&tty),
            "the output half must be attempted even after the input half failed"
        );

        prime(&tty, true);
        assert!(
            set_state(&tty, not_a_terminal(), &target).is_err(),
            "a failing output half must be reported"
        );
        assert!(!opost(&tty), "the input half must still be applied");
    }

    #[test]
    fn get_state_errors_when_neither_half_is_a_terminal() {
        assert!(get_state(not_a_terminal(), not_a_terminal()).is_err());
    }

    /// A half can pass `tcgetattr` and then fail `tcsetattr` -- a pty whose
    /// master closed mid-session, say. The other half is already raw by then,
    /// and `make_raw_mode` consumes the only copy of the pre-call state on its
    /// way out, so without the write-back that half is raw forever.
    #[test]
    fn a_failed_half_does_not_strand_the_half_that_succeeded() {
        let (Some((_ma, a)), Some((_mb, b))) = (open_pty_pair(), open_pty_pair()) else {
            return;
        };
        prime(&a, true);
        prime(&b, true);
        let (before_a, before_b) = (attrs(&a), attrs(&b));

        // `get_state` sees a terminal, the raw `tcsetattr` does not, and the
        // write-back sees the terminal again.
        let pipe = not_a_terminal();
        let output = ScriptedFd::new(&[&b as &dyn AsFd, &pipe, &b]);

        let Err(err) = make_raw_mode(&a, &output) else {
            panic!("a pipe cannot be rawified, so make_raw_mode must fail");
        };
        // Most systems report a non-terminal descriptor as `ENOTTY`; Solaris
        // reports `EINVAL`. Either way the error has to come from the write to
        // the pipe rather than from anywhere else.
        assert!(
            matches!(err.raw_os_error(), Some(libc::ENOTTY | libc::EINVAL)),
            "expected a not-a-terminal error, got {err}"
        );
        assert_restored(&before_a, &a, "input half");
        assert_restored(&before_b, &b, "output half");
    }
}

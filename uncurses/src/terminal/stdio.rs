//! Direct stdio handles that mirror [`std::io::stdout`],
//! [`std::io::stderr`], and [`std::io::stdin`] but without `std`'s
//! `LineWriter` / `BufReader`.
//!
//! Behavior relative to the `std` counterparts:
//!
//! * **Process-wide singletons with a shared lock.** Every call to
//!   [`stdout`] / [`stderr`] / [`stdin`] returns a fresh handle that
//!   references the same underlying [`Mutex`]-guarded raw stream.
//!   Cloning a handle is free; only the [`Mutex`] is shared, so
//!   concurrent writes from multiple threads are serialised and do
//!   not interleave at the byte level.
//! * **No line buffering on writes.** A call to [`Write::write`] is
//!   forwarded to a single OS write call, regardless of any `\n`
//!   present in the buffer.
//! * **Unbuffered.** No reads or writes are batched. Wrap in
//!   [`std::io::BufWriter`] / [`std::io::BufReader`] to coalesce
//!   syscalls.
//! * **Same Windows console semantics.** When the inherited handle
//!   refers to a console, output is transcoded UTF-8 → UTF-16 and
//!   delivered through `WriteConsoleW`, and input is read with
//!   `ReadConsoleW` and transcoded UTF-16 → UTF-8 — matching what
//!   `std::io::Stdout` / `std::io::Stdin` do. UTF-8 sequences split
//!   across calls are carried forward via a 4-byte per-stream buffer.
//! * **Independent of `std`'s lock.** The shared [`Mutex`] used here
//!   is separate from the one inside [`std::io::Stdout`] /
//!   [`std::io::Stdin`], so concurrent `println!` / panic output may
//!   still interleave with ours. TUI applications that own the
//!   screen typically route diagnostic output away from stdout to
//!   avoid this.
//! * **Non-reentrant lock.** Writing to the same stream from inside
//!   a write call (for example, from a panic hook that prints to
//!   stdout while a write is in progress on the same thread) will
//!   deadlock. The std counterparts use a reentrant lock to avoid
//!   this; we accept the limitation in exchange for keeping the
//!   surface area small.
//!
//! Each [`Stdout`] / [`Stderr`] / [`Stdin`] is a borrowed view of the
//! inherited descriptor; dropping it does not close that descriptor.

use std::io::{self, IoSlice, Read, Write};
use std::sync::{Mutex, MutexGuard, OnceLock};

#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};

#[cfg(windows)]
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, RawHandle};

// ---------------------------------------------------------------------------
// Process-wide singletons
// ---------------------------------------------------------------------------

static STDOUT: OnceLock<Mutex<StdoutRaw>> = OnceLock::new();
static STDERR: OnceLock<Mutex<StderrRaw>> = OnceLock::new();
static STDIN: OnceLock<Mutex<StdinRaw>> = OnceLock::new();

fn stdout_lock() -> &'static Mutex<StdoutRaw> {
    STDOUT.get_or_init(|| Mutex::new(StdoutRaw(imp::out())))
}
fn stderr_lock() -> &'static Mutex<StderrRaw> {
    STDERR.get_or_init(|| Mutex::new(StderrRaw(imp::err())))
}
fn stdin_lock() -> &'static Mutex<StdinRaw> {
    STDIN.get_or_init(|| Mutex::new(StdinRaw(imp::input())))
}

/// Returns a handle to the inherited standard output stream.
pub fn stdout() -> Stdout {
    Stdout {
        inner: stdout_lock(),
    }
}

/// Returns a handle to the inherited standard error stream.
pub fn stderr() -> Stderr {
    Stderr {
        inner: stderr_lock(),
    }
}

/// Returns a handle to the inherited standard input stream.
pub fn stdin() -> Stdin {
    Stdin {
        inner: stdin_lock(),
    }
}

// ---------------------------------------------------------------------------
// Raw, unbuffered, unlocked streams (private)
// ---------------------------------------------------------------------------

struct StdoutRaw(imp::Output);
struct StderrRaw(imp::Output);
struct StdinRaw(imp::Input);

impl Write for StdoutRaw {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }
}

impl Write for StderrRaw {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }
}

impl Read for StdinRaw {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

// ---------------------------------------------------------------------------
// Public handle types
// ---------------------------------------------------------------------------

/// A handle to the inherited standard output stream.
///
/// See the [module documentation](self) for the behavior relative to
/// [`std::io::Stdout`].
#[derive(Clone, Copy)]
pub struct Stdout {
    inner: &'static Mutex<StdoutRaw>,
}

/// A handle to the inherited standard error stream.
///
/// See the [module documentation](self) for the behavior relative to
/// [`std::io::Stderr`].
#[derive(Clone, Copy)]
pub struct Stderr {
    inner: &'static Mutex<StderrRaw>,
}

/// A handle to the inherited standard input stream.
///
/// See the [module documentation](self) for the behavior relative to
/// [`std::io::Stdin`].
#[derive(Clone, Copy)]
pub struct Stdin {
    inner: &'static Mutex<StdinRaw>,
}

impl Stdout {
    /// Acquire the shared write lock for the lifetime of the returned
    /// guard. While the guard is held, no other handle to stdout —
    /// from any thread — can write through this module.
    pub fn lock(&self) -> StdoutLock<'static> {
        StdoutLock {
            guard: self.inner.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

impl Stderr {
    /// Acquire the shared write lock for the lifetime of the returned
    /// guard. While the guard is held, no other handle to stderr —
    /// from any thread — can write through this module.
    pub fn lock(&self) -> StderrLock<'static> {
        StderrLock {
            guard: self.inner.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

impl Stdin {
    /// Acquire the shared read lock for the lifetime of the returned
    /// guard. While the guard is held, no other handle to stdin —
    /// from any thread — can read through this module.
    pub fn lock(&self) -> StdinLock<'static> {
        StdinLock {
            guard: self.inner.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

// ---------------------------------------------------------------------------
// Lock guards
// ---------------------------------------------------------------------------

/// A locked, exclusive reference to [`Stdout`]. Acquired via
/// [`Stdout::lock`].
pub struct StdoutLock<'a> {
    guard: MutexGuard<'a, StdoutRaw>,
}

/// A locked, exclusive reference to [`Stderr`]. Acquired via
/// [`Stderr::lock`].
pub struct StderrLock<'a> {
    guard: MutexGuard<'a, StderrRaw>,
}

/// A locked, exclusive reference to [`Stdin`]. Acquired via
/// [`Stdin::lock`].
pub struct StdinLock<'a> {
    guard: MutexGuard<'a, StdinRaw>,
}

impl Write for StdoutLock<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.guard.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.guard.flush()
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.guard.write_vectored(bufs)
    }
}

impl Write for StderrLock<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.guard.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.guard.flush()
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.guard.write_vectored(bufs)
    }
}

impl Read for StdinLock<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.guard.read(buf)
    }
}

// ---------------------------------------------------------------------------
// Write/Read impls on the handle types
//
// Each call locks the shared mutex briefly, performs the I/O, and
// releases. Both `&mut self` and `&self` impls are provided so the
// handles can be used with `write!`/`writeln!` macros via either an
// owned binding or a shared reference.
// ---------------------------------------------------------------------------

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.lock().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.lock().flush()
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.lock().write_vectored(bufs)
    }
}

impl Write for &Stdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.lock().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.lock().flush()
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.lock().write_vectored(bufs)
    }
}

impl Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.lock().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.lock().flush()
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.lock().write_vectored(bufs)
    }
}

impl Write for &Stderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.lock().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.lock().flush()
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.lock().write_vectored(bufs)
    }
}

impl Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.lock().read(buf)
    }
}

impl Read for &Stdin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.lock().read(buf)
    }
}

// ---------------------------------------------------------------------------
// AsFd / AsRawFd / AsHandle / AsRawHandle
//
// Implemented on every handle and guard type. The underlying
// descriptor / handle is process-stable, so these are lock-free and
// always reference the inherited stream.
// ---------------------------------------------------------------------------

#[cfg(unix)]
const STDOUT_FD: RawFd = libc::STDOUT_FILENO;
#[cfg(unix)]
const STDERR_FD: RawFd = libc::STDERR_FILENO;
#[cfg(unix)]
const STDIN_FD: RawFd = libc::STDIN_FILENO;

#[cfg(unix)]
macro_rules! impl_as_fd {
    ($ty:ty, $fd:expr) => {
        impl AsFd for $ty {
            fn as_fd(&self) -> BorrowedFd<'_> {
                // SAFETY: the standard descriptors are inherited from
                // the parent and remain valid for the lifetime of the
                // process; the returned borrow is bounded by `&self`.
                unsafe { BorrowedFd::borrow_raw($fd) }
            }
        }
        impl AsRawFd for $ty {
            fn as_raw_fd(&self) -> RawFd {
                $fd
            }
        }
    };
}

#[cfg(unix)]
impl_as_fd!(Stdout, STDOUT_FD);
#[cfg(unix)]
impl_as_fd!(Stderr, STDERR_FD);
#[cfg(unix)]
impl_as_fd!(Stdin, STDIN_FD);
#[cfg(unix)]
impl_as_fd!(StdoutLock<'_>, STDOUT_FD);
#[cfg(unix)]
impl_as_fd!(StderrLock<'_>, STDERR_FD);
#[cfg(unix)]
impl_as_fd!(StdinLock<'_>, STDIN_FD);

#[cfg(windows)]
macro_rules! impl_as_handle {
    ($ty:ty, $which:expr) => {
        impl AsHandle for $ty {
            fn as_handle(&self) -> BorrowedHandle<'_> {
                // SAFETY: the standard handles are inherited from the
                // parent and remain valid for the lifetime of the
                // process; the returned borrow is bounded by `&self`.
                let h = unsafe { ::windows_sys::Win32::System::Console::GetStdHandle($which) };
                unsafe { BorrowedHandle::borrow_raw(h as _) }
            }
        }
        impl AsRawHandle for $ty {
            fn as_raw_handle(&self) -> RawHandle {
                let h = unsafe { ::windows_sys::Win32::System::Console::GetStdHandle($which) };
                h as RawHandle
            }
        }
    };
}

#[cfg(windows)]
impl_as_handle!(
    Stdout,
    ::windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE
);
#[cfg(windows)]
impl_as_handle!(
    Stderr,
    ::windows_sys::Win32::System::Console::STD_ERROR_HANDLE
);
#[cfg(windows)]
impl_as_handle!(
    Stdin,
    ::windows_sys::Win32::System::Console::STD_INPUT_HANDLE
);
#[cfg(windows)]
impl_as_handle!(
    StdoutLock<'_>,
    ::windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE
);
#[cfg(windows)]
impl_as_handle!(
    StderrLock<'_>,
    ::windows_sys::Win32::System::Console::STD_ERROR_HANDLE
);
#[cfg(windows)]
impl_as_handle!(
    StdinLock<'_>,
    ::windows_sys::Win32::System::Console::STD_INPUT_HANDLE
);

// ---------------------------------------------------------------------------
// Unix implementation
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::mem::ManuallyDrop;
    use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, RawFd};

    pub(super) fn out() -> Output {
        Output::new(libc::STDOUT_FILENO)
    }
    pub(super) fn err() -> Output {
        Output::new(libc::STDERR_FILENO)
    }
    pub(super) fn input() -> Input {
        Input::new(libc::STDIN_FILENO)
    }

    pub(super) struct Output {
        // Borrowed view of an inherited fd; `ManuallyDrop` keeps `File`
        // from closing the descriptor when the wrapper is dropped.
        file: ManuallyDrop<File>,
    }

    pub(super) struct Input {
        file: ManuallyDrop<File>,
    }

    impl Output {
        fn new(fd: RawFd) -> Self {
            // SAFETY: fd 1 / 2 are inherited from the parent process
            // and remain valid for its lifetime; `ManuallyDrop`
            // prevents `File::drop` from calling `close(2)`.
            Self {
                file: ManuallyDrop::new(unsafe { File::from_raw_fd(fd) }),
            }
        }
    }

    impl Input {
        fn new(fd: RawFd) -> Self {
            // SAFETY: see `Output::new`.
            Self {
                file: ManuallyDrop::new(unsafe { File::from_raw_fd(fd) }),
            }
        }
    }

    impl Write for Output {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            (&*self.file).write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            (&*self.file).flush()
        }
        fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
            (&*self.file).write_vectored(bufs)
        }
    }

    impl Read for Input {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            (&*self.file).read(buf)
        }
    }

    impl AsFd for Output {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.file.as_fd()
        }
    }
    impl AsRawFd for Output {
        fn as_raw_fd(&self) -> RawFd {
            self.file.as_raw_fd()
        }
    }
    impl AsFd for Input {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.file.as_fd()
        }
    }
    impl AsRawFd for Input {
        fn as_raw_fd(&self) -> RawFd {
            self.file.as_raw_fd()
        }
    }
}

// ---------------------------------------------------------------------------
// Windows implementation
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use std::io::{self, IoSlice, Read, Write};
    use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, RawHandle};
    use std::ptr;
    use std::sync::{Mutex, OnceLock};

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_TYPE_CHAR, GetFileType, ReadFile, WriteFile,
    };
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, ReadConsoleW, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, WriteConsoleW,
    };

    // ---- Output ----------------------------------------------------------

    pub(super) fn out() -> Output {
        Output::new(STD_OUTPUT_HANDLE, &OUT_STATE)
    }
    pub(super) fn err() -> Output {
        Output::new(STD_ERROR_HANDLE, &ERR_STATE)
    }

    static OUT_STATE: OnceLock<Mutex<PartialUtf8>> = OnceLock::new();
    static ERR_STATE: OnceLock<Mutex<PartialUtf8>> = OnceLock::new();
    static IN_STATE: OnceLock<Mutex<PartialUtf8>> = OnceLock::new();

    pub(super) struct Output {
        which: u32,
        state: &'static OnceLock<Mutex<PartialUtf8>>,
    }

    impl Output {
        fn new(which: u32, state: &'static OnceLock<Mutex<PartialUtf8>>) -> Self {
            Self { which, state }
        }
        fn raw_handle(&self) -> HANDLE {
            // SAFETY: GetStdHandle returns the inherited process handle
            // or a null/invalid sentinel; callers downstream check for
            // failure via WriteFile/WriteConsoleW returning 0.
            unsafe { GetStdHandle(self.which) }
        }
        fn state(&self) -> &'static Mutex<PartialUtf8> {
            self.state.get_or_init(|| Mutex::new(PartialUtf8::new()))
        }
    }

    impl Write for Output {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let h = self.raw_handle();
            if is_console(h) {
                write_console(h, &mut self.state().lock().unwrap(), buf)
            } else {
                write_file(h, buf)
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
            // No scatter syscall on Windows console writes; do the
            // simple thing: write the first non-empty slice.
            for b in bufs {
                if !b.is_empty() {
                    return self.write(b);
                }
            }
            Ok(0)
        }
    }

    impl AsHandle for Output {
        fn as_handle(&self) -> BorrowedHandle<'_> {
            // SAFETY: the returned BorrowedHandle is tied to `&self`
            // and never outlives the process-inherited handle.
            unsafe { BorrowedHandle::borrow_raw(self.raw_handle() as _) }
        }
    }
    impl AsRawHandle for Output {
        fn as_raw_handle(&self) -> RawHandle {
            self.raw_handle() as RawHandle
        }
    }

    // ---- Input -----------------------------------------------------------

    pub(super) fn input() -> Input {
        Input
    }

    pub(super) struct Input;

    impl Input {
        fn raw_handle(&self) -> HANDLE {
            unsafe { GetStdHandle(STD_INPUT_HANDLE) }
        }
        fn state(&self) -> &'static Mutex<PartialUtf8> {
            IN_STATE.get_or_init(|| Mutex::new(PartialUtf8::new()))
        }
    }

    impl Read for Input {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let h = self.raw_handle();
            if is_console(h) {
                read_console(h, &mut self.state().lock().unwrap(), buf)
            } else {
                read_file(h, buf)
            }
        }
    }

    impl AsHandle for Input {
        fn as_handle(&self) -> BorrowedHandle<'_> {
            unsafe { BorrowedHandle::borrow_raw(self.raw_handle() as _) }
        }
    }
    impl AsRawHandle for Input {
        fn as_raw_handle(&self) -> RawHandle {
            self.raw_handle() as RawHandle
        }
    }

    // ---- shared helpers --------------------------------------------------

    /// Carry-over for UTF-8 sequences split across calls.
    ///
    /// On the write side: holds the trailing bytes of a partial UTF-8
    /// codepoint from the previous call so the next call can prepend
    /// them.
    ///
    /// On the read side: holds the trailing bytes of a UTF-8 codepoint
    /// that did not fit in the caller's buffer so the next call can
    /// return them first. Sized at 8 bytes so up to two 4-byte
    /// codepoints worth of overflow can be queued (see `read_console`
    /// for the sizing argument).
    struct PartialUtf8 {
        buf: [u8; 8],
        len: u8,
    }

    impl PartialUtf8 {
        fn new() -> Self {
            Self {
                buf: [0; 8],
                len: 0,
            }
        }
    }

    fn is_console(h: HANDLE) -> bool {
        if h.is_null() || (h as isize) == -1 {
            return false;
        }
        if unsafe { GetFileType(h) } != FILE_TYPE_CHAR {
            return false;
        }
        let mut mode: u32 = 0;
        unsafe { GetConsoleMode(h, &mut mode) != 0 }
    }

    fn write_file(h: HANDLE, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // WriteFile takes a u32 length; cap to a safe chunk.
        let len = buf.len().min(u32::MAX as usize) as u32;
        let mut written: u32 = 0;
        let ok = unsafe { WriteFile(h, buf.as_ptr(), len, &mut written, ptr::null_mut()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(written as usize)
    }

    fn read_file(h: HANDLE, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize) as u32;
        let mut read: u32 = 0;
        let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), len, &mut read, ptr::null_mut()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(read as usize)
    }

    /// Console write path.
    ///
    /// Validates `buf` (concatenated with any carried-over partial
    /// UTF-8 prefix) as UTF-8, transcodes the longest valid prefix to
    /// UTF-16, and writes it with `WriteConsoleW`. Saves any trailing
    /// 1..=3 bytes of an incomplete final codepoint into `state` so a
    /// future call can complete it.
    ///
    /// Returns the number of bytes from the *caller's* `buf` that were
    /// consumed.
    fn write_console(h: HANDLE, state: &mut PartialUtf8, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        // 1. Try to complete any saved partial sequence with the
        //    leading bytes of `buf`. A UTF-8 codepoint is at most 4
        //    bytes, so we need to append at most 3 leading bytes.
        let mut leading_consumed: usize = 0;
        if state.len > 0 {
            while leading_consumed < buf.len() && (state.len as usize) < 4 {
                state.buf[state.len as usize] = buf[leading_consumed];
                state.len += 1;
                leading_consumed += 1;

                // After each append: if the saved bytes form a valid
                // UTF-8 codepoint, emit it and clear the buffer.
                if let Ok(s) = std::str::from_utf8(&state.buf[..state.len as usize]) {
                    let c = s.chars().next().unwrap();
                    let mut units = [0u16; 2];
                    let encoded = c.encode_utf16(&mut units);
                    write_utf16_console_all(h, encoded)?;
                    state.len = 0;
                    break;
                }

                // 4 bytes that still aren't valid UTF-8: bail out with
                // a replacement character.
                if state.len == 4 {
                    write_utf16_console_all(h, &['\u{FFFD}' as u16])?;
                    state.len = 0;
                    break;
                }
            }
            if state.len > 0 {
                // Still incomplete; we've consumed everything we were
                // given but produced no caller-visible codepoint yet.
                return Ok(leading_consumed);
            }
        }

        let rest = &buf[leading_consumed..];
        if rest.is_empty() {
            return Ok(leading_consumed);
        }

        // 2. Find the longest valid UTF-8 prefix of `rest`.
        match std::str::from_utf8(rest) {
            Ok(s) => {
                write_utf8_to_console(h, s)?;
                Ok(leading_consumed + rest.len())
            }
            Err(e) => {
                let v = e.valid_up_to();
                // SAFETY: `v` is a valid UTF-8 boundary as reported by
                // `Utf8Error::valid_up_to`.
                let valid = unsafe { std::str::from_utf8_unchecked(&rest[..v]) };
                if !valid.is_empty() {
                    write_utf8_to_console(h, valid)?;
                }
                match e.error_len() {
                    Some(err_len) => {
                        // Truly invalid bytes; emit a single
                        // replacement and report progress past them.
                        write_utf16_console_all(h, &['\u{FFFD}' as u16])?;
                        Ok(leading_consumed + v + err_len)
                    }
                    None => {
                        // Trailing partial codepoint; stash 1..=3
                        // bytes for the next call to complete.
                        let trailing = &rest[v..];
                        state.buf[..trailing.len()].copy_from_slice(trailing);
                        state.len = trailing.len() as u8;
                        Ok(leading_consumed + rest.len())
                    }
                }
            }
        }
    }

    /// Transcode `s` to UTF-16 and write it with `WriteConsoleW`,
    /// looping until every code unit is delivered.
    fn write_utf8_to_console(h: HANDLE, s: &str) -> io::Result<()> {
        // Use a small stack buffer and refill it in chunks to avoid an
        // unbounded heap allocation for very large writes.
        const CHUNK: usize = 1024;
        let mut buf = [0u16; CHUNK];
        let mut idx = 0;
        for c in s.chars() {
            let need = c.len_utf16();
            if idx + need > CHUNK {
                write_utf16_console_all(h, &buf[..idx])?;
                idx = 0;
            }
            // Encode directly into the buffer.
            let written = c.encode_utf16(&mut buf[idx..idx + need]).len();
            idx += written;
        }
        if idx > 0 {
            write_utf16_console_all(h, &buf[..idx])?;
        }
        Ok(())
    }

    /// Single `WriteConsoleW` call; returns the number of UTF-16 code
    /// units written, or an error.
    fn write_utf16_console(h: HANDLE, data: &[u16]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let mut written: u32 = 0;
        let ok = unsafe {
            WriteConsoleW(
                h,
                data.as_ptr(),
                data.len() as u32,
                &mut written,
                ptr::null(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(written as usize)
    }

    /// `WriteConsoleW` looped until the entire slice is consumed.
    fn write_utf16_console_all(h: HANDLE, mut data: &[u16]) -> io::Result<()> {
        while !data.is_empty() {
            let n = write_utf16_console(h, data)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "WriteConsoleW returned 0 with non-empty buffer",
                ));
            }
            data = &data[n..];
        }
        Ok(())
    }

    /// Console read path.
    ///
    /// Drains any UTF-8 bytes previously stashed (overflow from the
    /// last read), then calls `ReadConsoleW` for a fresh batch and
    /// transcodes UTF-16 → UTF-8 into the caller's buffer. If the
    /// caller's buffer is too small for a decoded codepoint, the
    /// remaining bytes are saved in `state` for the next call.
    ///
    /// Sizing: each UTF-16 code unit decodes to at most 3 UTF-8 bytes
    /// (BMP), and a surrogate pair (2 units) decodes to 4 UTF-8 bytes
    /// total. We cap the request at `(buf.len() + 4) / 3` units, with
    /// a floor of 2 so a surrogate pair can be paired in a single
    /// read. That bounds the produced UTF-8 byte count at
    /// `request * 3`, which never exceeds `buf.len() + stash_cap`
    /// (`stash_cap == 8`) for `buf.len() >= 1`.
    fn read_console(h: HANDLE, state: &mut PartialUtf8, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        // 1. Drain anything we stashed from a previous call.
        if state.len > 0 {
            let n = (state.len as usize).min(buf.len());
            buf[..n].copy_from_slice(&state.buf[..n]);
            let rem = state.len as usize - n;
            if rem > 0 {
                state.buf.copy_within(n..n + rem, 0);
            }
            state.len = rem as u8;
            return Ok(n);
        }

        // 2. Read a bounded number of UTF-16 code units so that the
        //    decoded UTF-8 byte count fits in `buf` plus the stash.
        const MAX_UNITS: usize = 256;
        let want = ((buf.len() + 4) / 3).clamp(2, MAX_UNITS);
        let mut units = [0u16; MAX_UNITS];
        let mut read_units: u32 = 0;
        let ok = unsafe {
            ReadConsoleW(
                h,
                units.as_mut_ptr().cast(),
                want as u32,
                &mut read_units,
                ptr::null(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if read_units == 0 {
            return Ok(0);
        }
        let read_units = read_units as usize;

        // 3. Decode all received UTF-16 units to UTF-8 in one pass into
        //    a stack scratch buffer; then split between caller `buf`
        //    and `state.buf`.
        //
        // Worst case decoded size: `read_units * 3` for an all-BMP
        // 3-byte-UTF-8 stream. `read_units <= MAX_UNITS == 256`, so
        // 768 bytes suffices.
        let mut scratch = [0u8; MAX_UNITS * 3];
        let mut s_len = 0usize;
        for r in char::decode_utf16(units[..read_units].iter().copied()) {
            let c = r.unwrap_or('\u{FFFD}');
            let n = c.len_utf8();
            c.encode_utf8(&mut scratch[s_len..s_len + n]);
            s_len += n;
        }

        let n = s_len.min(buf.len());
        buf[..n].copy_from_slice(&scratch[..n]);
        let overflow = s_len - n;
        if overflow > 0 {
            debug_assert!(overflow <= state.buf.len());
            let cap = state.buf.len();
            let take = overflow.min(cap);
            state.buf[..take].copy_from_slice(&scratch[n..n + take]);
            state.len = take as u8;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn stdout_handle_constructible() {
        let _ = stdout();
        let _ = stderr();
        let _ = stdin();
    }

    #[test]
    fn stdout_write_empty_ok() {
        // Writing zero bytes must be a no-op regardless of attachment.
        let mut out = stdout();
        assert!(matches!(out.write(b""), Ok(0)));
    }

    #[cfg(unix)]
    #[test]
    fn handles_expose_inherited_fds() {
        use std::os::fd::AsRawFd;
        assert_eq!(stdin().as_raw_fd(), libc::STDIN_FILENO);
        assert_eq!(stdout().as_raw_fd(), libc::STDOUT_FILENO);
        assert_eq!(stderr().as_raw_fd(), libc::STDERR_FILENO);
    }

    #[cfg(windows)]
    #[test]
    fn handles_match_getstdhandle() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        unsafe {
            assert_eq!(
                stdin().as_raw_handle() as isize,
                GetStdHandle(STD_INPUT_HANDLE) as isize
            );
            assert_eq!(
                stdout().as_raw_handle() as isize,
                GetStdHandle(STD_OUTPUT_HANDLE) as isize
            );
            assert_eq!(
                stderr().as_raw_handle() as isize,
                GetStdHandle(STD_ERROR_HANDLE) as isize
            );
        }
    }
}

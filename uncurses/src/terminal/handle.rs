//! [`Terminal`] — an owned handle that manages a terminal device.
//!
//! A `Terminal` bundles a readable input half, a writable output half, a
//! snapshot of the process [`Env`], and the raw-mode lifecycle for the
//! device. It implements [`Read`] and [`Write`] (over the input and
//! output halves) and offers raw-mode / size / environment helpers.
//!
//! It is **not** `Copy`: it caches the terminal state across
//! [`make_raw`](Terminal::make_raw) so [`restore`](Terminal::restore)
//! can revert it without a `State` argument — the same self-managed
//! teardown pattern as [`Screen::reset`](crate::screen::Screen::reset).
//! There is no `Drop`; restore explicitly.
//!
//! Feed [`Screen`] and [`EventSource`] from the `Copy` halves with
//! [`output`](Terminal::output) / [`input`](Terminal::input), and keep
//! the `Terminal` for the raw-mode lifecycle:
//!
//! ```no_run
//! use std::io::Write;
//! use uncurses::terminal::Terminal;
//! use uncurses::screen::Screen;
//! use uncurses::event::EventSource;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut term = Terminal::open()?; // owns fds + env; not Copy
//! let _prev = term.make_raw()?;     // caches the prior state
//! let mut screen = Screen::new(term.output(), term.window_size()?);
//! let mut source = EventSource::new(term.input())?;
//! // ... draw to `screen`, read from `source` ...
//! screen.reset();
//! screen.flush()?;
//! term.restore()?; // revert to the cached state
//! # Ok(())
//! # }
//! ```
//!
//! [`Screen`]: crate::screen::Screen
//! [`EventSource`]: crate::event::EventSource

use std::io::{self, Read, Write};

#[cfg(unix)]
use std::os::fd::{AsFd, BorrowedFd};
#[cfg(windows)]
use std::os::windows::io::{AsHandle, BorrowedHandle};

use super::env::Env;
use super::raw::{self, State};
use super::size::{Winsize, get_window_size};
use super::stdio::{Stdin, Stdout, stdin, stdout};
use super::tty::{TtyInput, TtyOutput, open_tty};

/// An owned handle that manages a terminal device: a readable input
/// half, a writable output half, an [`Env`] snapshot, and the raw-mode
/// lifecycle.
///
/// `Terminal` implements [`Read`] (from the input) and [`Write`] (to the
/// output). It is **not** `Copy` — it caches the pre-raw state so
/// [`restore`](Self::restore) takes no argument. Build a [`Screen`] and a
/// [`EventSource`] from the `Copy` halves via [`output`](Self::output) /
/// [`input`](Self::input), and keep the `Terminal` itself for the
/// raw-mode lifecycle.
///
/// [`Screen`]: crate::screen::Screen
/// [`EventSource`]: crate::event::EventSource
pub struct Terminal<I, O> {
    input: I,
    output: O,
    /// State captured by the most recent [`make_raw`](Self::make_raw),
    /// applied (and cleared) by [`restore`](Self::restore).
    saved: Option<State>,
    env: Env,
}

impl Terminal<Stdin, Stdout> {
    /// A handle over the process stdio (`stdin` + `stdout`), snapshotting
    /// the process environment.
    pub fn stdio() -> Self {
        Self::new(stdin(), stdout(), Env::from_process())
    }
}

impl Terminal<TtyInput, TtyOutput> {
    /// Open the controlling terminal directly (`/dev/tty`, or
    /// `CONIN$`/`CONOUT$` on Windows) — useful when stdio is redirected.
    /// Snapshots the process environment.
    pub fn open() -> io::Result<Self> {
        let (input, output) = open_tty()?;
        Ok(Self::new(input, output, Env::from_process()))
    }
}

impl<I, O> Terminal<I, O> {
    /// The captured environment snapshot.
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// Look up an environment variable from the captured snapshot, or
    /// `None` if unset. Delegates to [`Env::get`].
    pub fn get_env(&self, key: &str) -> Option<String> {
        self.env.get(key)
    }

    /// Whether an environment variable is present with a non-empty value.
    /// Delegates to [`Env::has`].
    pub fn has_env(&self, key: &str) -> bool {
        self.env.has(key)
    }

    /// Copy of the input half — pass to
    /// [`EventSource::new`](crate::event::EventSource::new).
    pub fn input(&self) -> I
    where
        I: Copy,
    {
        self.input
    }

    /// Copy of the output half — pass to
    /// [`Screen::new`](crate::screen::Screen::new).
    pub fn output(&self) -> O
    where
        O: Copy,
    {
        self.output
    }

    /// Consume the handle, yielding the owned input and output halves.
    pub fn into_halves(self) -> (I, O) {
        (self.input, self.output)
    }
}

impl<I: Read, O> Read for Terminal<I, O> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

impl<I, O: Write> Write for Terminal<I, O> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

#[cfg(unix)]
impl<I: AsFd, O> AsFd for Terminal<I, O> {
    /// Borrows the **input** descriptor.
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.input.as_fd()
    }
}

#[cfg(windows)]
impl<I: AsHandle, O> AsHandle for Terminal<I, O> {
    /// Borrows the **input** handle.
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.input.as_handle()
    }
}

#[cfg(unix)]
impl<I: AsFd, O: AsFd> Terminal<I, O> {
    /// Build a handle from input and output halves (terminal
    /// descriptors) with an explicit [`Env`]. The environment is not
    /// assumed to relate to the given halves; use [`stdio`](Self::stdio)
    /// / [`open`](Self::open) to snapshot the process environment
    /// automatically.
    pub fn new(input: I, output: O, env: Env) -> Self {
        Self {
            input,
            output,
            saved: None,
            env,
        }
    }

    /// Put the terminal into raw mode. Caches the prior state for
    /// [`restore`](Self::restore) and also returns it.
    pub fn make_raw(&mut self) -> io::Result<State> {
        let prev = raw::make_raw_mode(&self.input, &self.output)?;
        self.saved = Some(prev.clone());
        Ok(prev)
    }

    /// Restore the state cached by the most recent
    /// [`make_raw`](Self::make_raw), clearing the cache. A no-op if
    /// nothing is cached.
    pub fn restore(&mut self) -> io::Result<()> {
        match self.saved.take() {
            Some(state) => raw::set_state(&self.input, &self.output, &state),
            None => Ok(()),
        }
    }

    /// Snapshot the current terminal mode.
    pub fn get_state(&self) -> io::Result<State> {
        raw::get_state(&self.input, &self.output)
    }

    /// Apply a previously snapshotted terminal mode.
    pub fn set_state(&self, state: &State) -> io::Result<()> {
        raw::set_state(&self.input, &self.output, state)
    }

    /// Whether the input and output halves are each connected to a
    /// terminal, as `(input, output)`.
    pub fn is_terminal(&self) -> (bool, bool) {
        (
            raw::is_terminal(&self.input),
            raw::is_terminal(&self.output),
        )
    }

    /// Query the current window size (`TIOCGWINSZ`).
    ///
    /// Tries the output half first, then falls back to the input half if
    /// the output query fails (for example when stdout is redirected to a
    /// pipe while stdin is still attached to the terminal).
    pub fn window_size(&self) -> io::Result<Winsize> {
        get_window_size(&self.output).or_else(|_| get_window_size(&self.input))
    }
}

#[cfg(windows)]
impl<I: AsHandle, O: AsHandle> Terminal<I, O> {
    /// Build a handle from input and output halves (terminal handles)
    /// with an explicit [`Env`]. The environment is not assumed to
    /// relate to the given halves; use [`stdio`](Self::stdio) /
    /// [`open`](Self::open) to snapshot the process environment
    /// automatically.
    pub fn new(input: I, output: O, env: Env) -> Self {
        Self {
            input,
            output,
            saved: None,
            env,
        }
    }

    /// Put the terminal into raw mode. Caches the prior state for
    /// [`restore`](Self::restore) and also returns it.
    pub fn make_raw(&mut self) -> io::Result<State> {
        let prev = raw::make_raw_mode(&self.input, &self.output)?;
        self.saved = Some(prev.clone());
        Ok(prev)
    }

    /// Restore the state cached by the most recent
    /// [`make_raw`](Self::make_raw), clearing the cache. A no-op if
    /// nothing is cached.
    pub fn restore(&mut self) -> io::Result<()> {
        match self.saved.take() {
            Some(state) => raw::set_state(&self.input, &self.output, &state),
            None => Ok(()),
        }
    }

    /// Snapshot the current terminal mode.
    pub fn get_state(&self) -> io::Result<State> {
        raw::get_state(&self.input, &self.output)
    }

    /// Apply a previously snapshotted terminal mode.
    pub fn set_state(&self, state: &State) -> io::Result<()> {
        raw::set_state(&self.input, &self.output, state)
    }

    /// Whether the input and output halves are each connected to a
    /// terminal, as `(input, output)`.
    pub fn is_terminal(&self) -> (bool, bool) {
        (
            raw::is_terminal(&self.input),
            raw::is_terminal(&self.output),
        )
    }

    /// Query the current window size from the output's console screen
    /// buffer (`GetConsoleScreenBufferInfo`). Pixel dimensions are
    /// unavailable on this platform and report as `0`.
    pub fn window_size(&self) -> io::Result<Winsize> {
        get_window_size(&self.output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_helpers_delegate_to_snapshot() {
        let env = Env::from_pairs([("TERM", "xterm-256color"), ("NO_COLOR", "1"), ("EMPTY", "")]);
        let term = Terminal::new(stdin(), stdout(), env);

        assert_eq!(term.get_env("TERM").as_deref(), Some("xterm-256color"));
        assert!(term.has_env("TERM"));
        assert!(!term.has_env("EMPTY")); // present but empty
        assert!(!term.has_env("MISSING"));
        // The full snapshot is reachable for richer queries (e.g. bool).
        assert_eq!(term.env().get("NO_COLOR").as_deref(), Some("1"));
        assert!(term.env().bool("NO_COLOR"));
    }
}

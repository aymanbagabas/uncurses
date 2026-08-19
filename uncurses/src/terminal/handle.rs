//! [`Terminal`] — a typed input/output handle with raw-mode state.
//!
//! A `Terminal<I, O>` bundles a readable input half, a writable output half, a
//! an [`Env`], and one optional saved raw-mode [`State`]. It implements
//! [`Read`] and [`Write`] by delegating to those halves, so it can be used
//! directly for byte-level terminal control or split into halves for a
//! renderer and an event source.
//!
//! ## Raw-mode ownership
//!
//! `Terminal` is not `Copy` because it owns the saved state used by
//! [`restore`](Terminal::restore). [`make_raw`](Terminal::make_raw) stores the
//! pre-raw state inside the handle and returns a clone to the caller.
//! [`restore`](Terminal::restore) applies and clears that cached state. There
//! is no `Drop` restoration; callers must restore explicitly.
//!
//! ## Choosing handles
//!
//! [`Terminal::stdio`] uses inherited stdin/stdout. [`Terminal::open`] opens
//! the controlling terminal directly, which keeps terminal I/O available when
//! stdio is redirected. For custom handles, [`Terminal::new`] pairs any
//! platform terminal input/output types with an explicit environment.
//!
//! ```rust,ignore
//! use std::io::Write;
//! use uncurses::buffer::TextBuffer;
//! use uncurses::event::EventSource;
//! use uncurses::terminal::Terminal;
//! use uncurses::text::Encode;
//!
//! let mut term = Terminal::open()?;
//! let _saved = term.make_raw()?;
//! let size = term.get_window_size()?;
//! let mut frame = TextBuffer::new(size.col, size.row);
//! let mut source = EventSource::new(term.input())?;
//!
//! // Paint `frame`, read events from `source`.
//! frame.encode(&mut term.output())?;
//! term.restore()?;
//! # Ok::<(), std::io::Error>(())
//! ```

use std::io::{self, Read, Write};

#[cfg(unix)]
use std::os::fd::{AsFd, BorrowedFd};
#[cfg(windows)]
use std::os::windows::io::{AsHandle, BorrowedHandle};

use super::env::{Env, ProcessEnv};
use super::raw::{self, State};
use super::size::{Winsize, get_window_size};
use super::stdio::{Stdin, Stdout, stdin, stdout};
use super::tty::{TtyInput, TtyOutput, open_tty};

/// Owned terminal handle pairing input, output, environment, and raw-mode
/// state.
///
/// `Terminal` implements [`Read`] from `I` and [`Write`] to `O`. The handle is
/// generic so callers can use inherited stdio, the controlling terminal, or
/// test doubles, while sharing one raw-mode and window-size API on supported
/// platforms.
///
/// [`EventSource`]: crate::event::EventSource
pub struct Terminal<I, O> {
    input: I,
    output: O,
    /// State captured by the most recent [`make_raw`](Self::make_raw),
    /// applied (and cleared) by [`restore`](Self::restore).
    saved: Option<State>,
    env: Box<dyn Env>,
}

impl Terminal<Stdin, Stdout> {
    /// Create a terminal over inherited standard input and output.
    ///
    /// The returned handle uses [`stdin`] for input, [`stdout`] for output, and
    /// [`ProcessEnv`] for its environment. Use this when the
    /// process is expected to be connected directly to the terminal.
    ///
    /// # Returns
    ///
    /// A `Terminal<Stdin, Stdout>` with no saved raw-mode state.
    ///
    /// # Errors and panics
    ///
    /// This constructor does not fail or intentionally panic.
    ///
    /// # Usage note
    ///
    /// If stdin or stdout may be redirected, prefer [`Terminal::open`] to open
    /// the controlling terminal directly.
    pub fn stdio() -> Self {
        Self::new(stdin(), stdout(), ProcessEnv)
    }
}

impl Terminal<TtyInput, TtyOutput> {
    /// Open the controlling terminal directly.
    ///
    /// On Unix this opens `/dev/tty` for both input and output. On Windows it
    /// opens `CONIN$` for input and `CONOUT$` for output. The returned
    /// `Terminal` reads the live process environment through [`ProcessEnv`].
    ///
    /// # Returns
    ///
    /// A `Terminal<TtyInput, TtyOutput>` backed by the controlling terminal.
    ///
    /// # Errors
    ///
    /// Returns the error from `open_tty` if the process has no controlling
    /// terminal or if the platform device cannot be opened.
    ///
    /// # Panics
    ///
    /// This function does not intentionally panic.
    pub fn open() -> io::Result<Self> {
        let (input, output) = open_tty()?;
        Ok(Self::new(input, output, ProcessEnv))
    }
}

impl<I, O> Terminal<I, O> {
    /// Build a terminal directly from its parts, without touching any fd.
    ///
    /// Test-only constructor used to assemble a [`Terminal`] over in-memory
    /// or non-tty handles (for example a `Vec<u8>` output) where the
    /// fd-bound [`new`](Self::new) cannot apply. No raw-mode state is
    /// captured.
    #[cfg(test)]
    pub(crate) fn from_parts(input: I, output: O, env: impl Env + 'static) -> Self {
        Self {
            input,
            output,
            saved: None,
            env: Box::new(env),
        }
    }

    /// Return the terminal's environment.
    ///
    /// Whether lookups see later changes to the process environment depends on
    /// the [`Env`] the terminal was built with.
    ///
    /// # Returns
    ///
    /// A shared reference to the terminal's [`Env`].
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn env(&self) -> &dyn Env {
        self.env.as_ref()
    }

    /// Look up an environment variable.
    ///
    /// # Parameters
    ///
    /// * `key` — environment variable name.
    ///
    /// # Returns
    ///
    /// The value for `key`, or `None` if it is absent.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn get_env(&self, key: &str) -> Option<String> {
        self.env.get(key)
    }

    /// Return whether an environment variable is present and non-empty.
    ///
    /// # Parameters
    ///
    /// * `key` — environment variable name.
    ///
    /// # Returns
    ///
    /// `true` when `key` is present with a non-empty value.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn has_env(&self, key: &str) -> bool {
        self.env.has(key)
    }

    /// Return a copy of the input half.
    ///
    /// This is available only when `I: Copy`, which is true for the standard
    /// and controlling-terminal handle types provided by this module. Use it to
    /// pass input to [`EventSource::new`](crate::event::EventSource::new) while
    /// retaining the `Terminal` for raw-mode restoration.
    ///
    /// # Returns
    ///
    /// A copy of the input handle.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn input(&self) -> I
    where
        I: Copy,
    {
        self.input
    }

    /// Return a copy of the output half.
    ///
    /// This is available only when `O: Copy`, which is true for the standard
    /// and controlling-terminal output types provided by this module. Use it to
    /// pass output to a renderer such as [`TextBuffer`](crate::buffer::TextBuffer) while
    /// retaining the `Terminal` for raw-mode restoration.
    ///
    /// # Returns
    ///
    /// A copy of the output handle.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn output(&self) -> O
    where
        O: Copy,
    {
        self.output
    }

    /// Consume the terminal and return its input and output halves.
    ///
    /// Any cached raw-mode state is dropped without being applied. Restore
    /// before calling this method if the terminal is in raw mode.
    ///
    /// # Returns
    ///
    /// `(input, output)`.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
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
    /// Borrow the input descriptor.
    ///
    /// This exposes the input half, not the output half. Use
    /// [`output`](Self::output) and borrow that handle when an output
    /// descriptor is required.
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.input.as_fd()
    }
}

#[cfg(windows)]
impl<I: AsHandle, O> AsHandle for Terminal<I, O> {
    /// Borrow the input handle.
    ///
    /// This exposes the input half, not the output half. Use
    /// [`output`](Self::output) and borrow that handle when an output handle is
    /// required.
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.input.as_handle()
    }
}

#[cfg(unix)]
impl<I: AsFd, O: AsFd> Terminal<I, O> {
    /// Build a terminal from input and output descriptors plus an [`Env`].
    ///
    /// The environment is stored exactly as provided and is not required to
    /// describe the given descriptors. Use [`Terminal::stdio`] or
    /// [`Terminal::open`] to use the process environment automatically.
    ///
    /// The two descriptors need not refer to the same device. Raw mode is
    /// applied to and restored from each one independently, so pairing two
    /// different terminals configures both, and pairing a terminal with a pipe
    /// configures only the terminal half.
    ///
    /// # Parameters
    ///
    /// * `input` — readable terminal descriptor.
    /// * `output` — writable terminal descriptor.
    /// * `env` — environment this terminal reads variables from.
    ///
    /// # Returns
    ///
    /// A `Terminal` with no saved raw-mode state.
    ///
    /// # Errors and panics
    ///
    /// This constructor does not fail or intentionally panic.
    pub fn new(input: I, output: O, env: impl Env + 'static) -> Self {
        Self {
            input,
            output,
            saved: None,
            env: Box::new(env),
        }
    }

    /// Put the terminal into raw mode and save the previous state.
    ///
    /// This calls `make_raw_mode` with the terminal's input and output
    /// descriptors. The returned pre-raw [`State`] is cloned into the terminal
    /// so [`restore`](Self::restore) can later apply it without an argument.
    ///
    /// # Returns
    ///
    /// The state that was active before raw mode was applied.
    ///
    /// # Errors
    ///
    /// Returns any error from reading the current state or applying the raw
    /// state.
    ///
    /// # Panics
    ///
    /// This method does not intentionally panic.
    pub fn make_raw(&mut self) -> io::Result<State> {
        let prev = raw::make_raw_mode(&self.input, &self.output)?;
        self.saved = Some(prev.clone());
        Ok(prev)
    }

    /// Restore the state cached by the most recent [`make_raw`](Self::make_raw).
    ///
    /// If a state is cached, it is applied with `set_state` and then
    /// cleared. If no state is cached, this is a no-op. The cache is kept when
    /// applying fails, so a failed restore can be retried rather than losing
    /// the only copy of the pre-raw state.
    ///
    /// # Returns
    ///
    /// `Ok(())` when no state was cached or restoration succeeded.
    ///
    /// # Errors
    ///
    /// Returns any error from applying the cached state.
    ///
    /// # Panics
    ///
    /// This method does not intentionally panic.
    pub fn restore(&mut self) -> io::Result<()> {
        if let Some(state) = self.saved.as_ref() {
            raw::set_state(&self.input, &self.output, state)?;
            self.saved = None;
        }
        Ok(())
    }

    /// Snapshot the current terminal mode.
    ///
    /// This reads the terminal state without modifying the cached state used by
    /// [`restore`](Self::restore).
    ///
    /// # Returns
    ///
    /// The current [`State`].
    ///
    /// # Errors
    ///
    /// Returns any error from `get_state`.
    ///
    /// # Panics
    ///
    /// This method does not intentionally panic.
    pub fn get_state(&self) -> io::Result<State> {
        raw::get_state(&self.input, &self.output)
    }

    /// Apply a previously snapshotted terminal mode.
    ///
    /// This does not update or clear the state cached by
    /// [`make_raw`](Self::make_raw). Use it for manual state management; use
    /// [`restore`](Self::restore) for the terminal-owned raw-mode lifecycle.
    /// `state` should have been read from this terminal's own descriptors, since
    /// a [`State`] records each half separately.
    ///
    /// # Parameters
    ///
    /// * `state` — state to apply to the terminal.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the state was applied.
    ///
    /// # Errors
    ///
    /// Returns any error from `set_state`.
    ///
    /// # Panics
    ///
    /// This method does not intentionally panic.
    pub fn set_state(&self, state: &State) -> io::Result<()> {
        raw::set_state(&self.input, &self.output, state)
    }

    /// Report whether the input and output halves are terminals.
    ///
    /// # Returns
    ///
    /// `(input_is_terminal, output_is_terminal)`.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn is_terminal(&self) -> (bool, bool) {
        (
            raw::is_terminal(&self.input),
            raw::is_terminal(&self.output),
        )
    }

    /// Query the current terminal window size.
    ///
    /// On Unix this tries the output descriptor first and falls back to the
    /// input descriptor if the output query fails. If both fail, the output
    /// descriptor's error is returned.
    ///
    /// # Returns
    ///
    /// The current window size in cells and, when reported by the platform,
    /// pixels.
    ///
    /// # Errors
    ///
    /// Returns an OS error if the size cannot be queried from either half.
    ///
    /// # Panics
    ///
    /// This method does not intentionally panic.
    pub fn get_window_size(&self) -> io::Result<Winsize> {
        get_window_size(&self.output).or_else(|e| get_window_size(&self.input).map_err(|_| e))
    }
}

#[cfg(windows)]
impl<I: AsHandle, O: AsHandle> Terminal<I, O> {
    /// Build a terminal from input and output handles plus an [`Env`].
    ///
    /// The environment is stored exactly as provided and is not required to
    /// describe the given handles. Use [`Terminal::stdio`] or
    /// [`Terminal::open`] to use the process environment automatically.
    ///
    /// # Parameters
    ///
    /// * `input` — readable console handle.
    /// * `output` — writable console handle.
    /// * `env` — environment this terminal reads variables from.
    ///
    /// # Returns
    ///
    /// A `Terminal` with no saved raw-mode state.
    ///
    /// # Errors and panics
    ///
    /// This constructor does not fail or intentionally panic.
    pub fn new(input: I, output: O, env: impl Env + 'static) -> Self {
        Self {
            input,
            output,
            saved: None,
            env: Box::new(env),
        }
    }

    /// Put the terminal into raw mode and save the previous state.
    ///
    /// This calls `make_raw_mode` with the terminal's input and output
    /// handles. The returned pre-raw [`State`] is cloned into the terminal so
    /// [`restore`](Self::restore) can later apply it without an argument.
    ///
    /// # Returns
    ///
    /// The state that was active before raw mode was applied.
    ///
    /// # Errors
    ///
    /// Returns any error from reading the current state or applying the raw
    /// state.
    ///
    /// # Panics
    ///
    /// This method does not intentionally panic.
    pub fn make_raw(&mut self) -> io::Result<State> {
        let prev = raw::make_raw_mode(&self.input, &self.output)?;
        self.saved = Some(prev.clone());
        Ok(prev)
    }

    /// Restore the state cached by the most recent [`make_raw`](Self::make_raw).
    ///
    /// If a state is cached, it is applied with `set_state` and then
    /// cleared. If no state is cached, this is a no-op. The cache is kept when
    /// applying fails, so a failed restore can be retried rather than losing
    /// the only copy of the pre-raw state.
    ///
    /// # Returns
    ///
    /// `Ok(())` when no state was cached or restoration succeeded.
    ///
    /// # Errors
    ///
    /// Returns any error from applying the cached state.
    ///
    /// # Panics
    ///
    /// This method does not intentionally panic.
    pub fn restore(&mut self) -> io::Result<()> {
        if let Some(state) = self.saved.as_ref() {
            raw::set_state(&self.input, &self.output, state)?;
            self.saved = None;
        }
        Ok(())
    }

    /// Snapshot the current terminal mode.
    ///
    /// This reads the console modes without modifying the cached state used by
    /// [`restore`](Self::restore).
    ///
    /// # Returns
    ///
    /// The current [`State`].
    ///
    /// # Errors
    ///
    /// Returns any error from `get_state`.
    ///
    /// # Panics
    ///
    /// This method does not intentionally panic.
    pub fn get_state(&self) -> io::Result<State> {
        raw::get_state(&self.input, &self.output)
    }

    /// Apply a previously snapshotted terminal mode.
    ///
    /// This does not update or clear the state cached by
    /// [`make_raw`](Self::make_raw). Use it for manual state management; use
    /// [`restore`](Self::restore) for the terminal-owned raw-mode lifecycle.
    /// `state` should have been read from this terminal's own descriptors, since
    /// a [`State`] records each half separately.
    ///
    /// # Parameters
    ///
    /// * `state` — state to apply to the terminal.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the state was applied.
    ///
    /// # Errors
    ///
    /// Returns any error from `set_state`.
    ///
    /// # Panics
    ///
    /// This method does not intentionally panic.
    pub fn set_state(&self, state: &State) -> io::Result<()> {
        raw::set_state(&self.input, &self.output, state)
    }

    /// Report whether the input and output halves are terminals.
    ///
    /// # Returns
    ///
    /// `(input_is_terminal, output_is_terminal)`.
    ///
    /// # Errors and panics
    ///
    /// This method does not fail or intentionally panic.
    pub fn is_terminal(&self) -> (bool, bool) {
        (
            raw::is_terminal(&self.input),
            raw::is_terminal(&self.output),
        )
    }

    /// Query the current terminal window size.
    ///
    /// On Windows this queries the output console screen buffer. Pixel
    /// dimensions are unavailable and are reported as `0`.
    ///
    /// # Returns
    ///
    /// The current visible console window size in cells.
    ///
    /// # Errors
    ///
    /// Returns an OS error if the output handle is not a console screen buffer
    /// or the size query fails.
    ///
    /// # Panics
    ///
    /// This method does not intentionally panic.
    pub fn get_window_size(&self) -> io::Result<Winsize> {
        get_window_size(&self.output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::EnvList;

    #[test]
    fn env_helpers_delegate_to_env() {
        let env =
            EnvList::from_pairs([("TERM", "xterm-256color"), ("NO_COLOR", "1"), ("EMPTY", "")]);
        let term = Terminal::new(stdin(), stdout(), env);

        assert_eq!(term.get_env("TERM").as_deref(), Some("xterm-256color"));
        assert!(term.has_env("TERM"));
        assert!(!term.has_env("EMPTY")); // present but empty
        assert!(!term.has_env("MISSING"));
        // The full environment is reachable for richer queries.
        assert_eq!(term.env().get("NO_COLOR").as_deref(), Some("1"));
    }

    /// A restore that fails must not consume the cached state: it is the only
    /// copy of the pre-raw attributes, and dropping it turns a transient
    /// failure into a terminal the caller can no longer put back.
    #[cfg(all(unix, not(target_os = "l4re")))]
    #[test]
    fn a_failed_restore_keeps_the_saved_state_for_a_retry() {
        use crate::testutil::{ScriptedFd, open_pty_pair, opost, prime};
        use std::os::fd::AsFd;

        let (Some((_ma, a)), Some((_mb, b))) = (open_pty_pair(), open_pty_pair()) else {
            return;
        };
        // A fresh pty's attributes are implementation-defined, so put `OPOST`
        // where the assertions below need it rather than assuming it.
        prime(&b, true);

        // The two `make_raw` borrows land on the pty, the first restore lands on
        // a pipe and fails, and the retry lands on the pty again.
        let pipe = std::io::pipe().expect("pipe").0;
        let output = ScriptedFd::new(&[&b as &dyn AsFd, &b, &pipe, &b]);
        let mut term = Terminal::new(&a, output, EnvList::from_pairs([("TERM", "dumb")]));

        term.make_raw().expect("raw mode");
        assert!(!opost(&b), "the output half must be raw");

        term.restore()
            .expect_err("a restore through a pipe cannot succeed");
        // Without the cached state this is a silent no-op that reports success.
        term.restore().expect("the retry restores");
        assert!(opost(&b), "the retry must put the output half back");
    }
}

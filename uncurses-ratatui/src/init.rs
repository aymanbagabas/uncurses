//! Setup and teardown helpers for the default backend.
//!
//! ## What this module provides
//!
//! The backend constructors are deliberately explicit: constructing an
//! [`UncursesBackend`] does not enter raw mode, hide the cursor, or switch
//! screens. This module layers the conventional application setup on top and
//! returns a ready-to-draw [`Terminal`].
//!
//! ## Fullscreen default
//!
//! [`try_init`] and [`init`] create a backend over process stdio, initialize
//! the wrapped [`Program`](uncurses::screen::Screen) with default
//! [`ProgramOptions`], enter the alternate screen, hide the cursor, and build a
//! [`Terminal`] with [`Viewport::Fullscreen`].
//!
//! ## Custom options and viewports
//!
//! [`try_init_with_options`] and [`init_with_options`] accept the exact
//! [`TerminalOptions`] and [`ProgramOptions`] to apply. Fullscreen and fixed
//! viewports enter the alternate screen. [`Viewport::Inline`] intentionally
//! stays on the main screen, resizes the screen buffer to the requested inline
//! height, and lets the backend translate absolute frame rows into that inline
//! buffer.
//!
//! ## Restoration
//!
//! [`try_restore`] calls [`UncursesBackend::restore`], which delegates to the
//! wrapped screen's pause/teardown path: staged terminal modes are reset,
//! cursor visibility and screen state are restored, pending bytes are flushed,
//! and raw mode is left. [`restore`] is the logging convenience wrapper for
//! applications that cannot use `?` during shutdown.
//!
//! ## Manual setup
//!
//! Use [`UncursesBackend::stdio`], [`UncursesBackend::open`],
//! [`UncursesBackend::new`], and [`UncursesBackend::init_with`] directly when
//! process stdio is not the desired terminal or setup must be interleaved with
//! other terminal operations.

use std::io;

use ratatui::{Terminal, TerminalOptions, Viewport};
use uncurses::program::ProgramOptions;
use uncurses::terminal::{Stdin, Stdout};

use crate::UncursesBackend;

/// Default terminal type returned by the setup helpers.
///
/// This is a [`Terminal`] whose backend is [`UncursesBackend`] over the
/// process standard input and output handles. Use this alias for APIs that
/// accept the value returned by [`init`], [`try_init`],
/// [`init_with_options`], or [`try_init_with_options`].
///
/// The alias itself performs no setup and cannot fail.
pub type DefaultTerminal = Terminal<UncursesBackend<Stdin, Stdout>>;

/// Initialize a fullscreen terminal over process stdio and panic on failure.
///
/// This is the infallible convenience wrapper around [`try_init`]. It is
/// useful for examples and applications that prefer startup failure to abort
/// immediately with a clear message.
///
/// ## Returns
///
/// A ready-to-draw [`DefaultTerminal`] using [`Viewport::Fullscreen`] and
/// default [`ProgramOptions`].
///
/// ## Panics
///
/// Panics with `failed to initialize terminal` if [`try_init`] returns an
/// error.
///
/// ## Usage note
///
/// Pair a successful call with [`restore`] or [`try_restore`] before exiting.
pub fn init() -> DefaultTerminal {
    try_init().expect("failed to initialize terminal")
}

/// Initialize a fullscreen terminal over process stdio.
///
/// This uses [`try_init_with_options`] with [`Viewport::Fullscreen`] and
/// [`ProgramOptions::default`]. Setup constructs an [`UncursesBackend`] over
/// stdio, initializes its screen, enters the alternate screen, hides the
/// cursor, records the fullscreen viewport, and builds the returned
/// [`Terminal`].
///
/// ## Returns
///
/// A ready-to-draw [`DefaultTerminal`].
///
/// ## Errors
///
/// Returns any error from opening or configuring stdio, entering raw mode,
/// applying screen setup, entering the alternate screen, hiding the cursor, or
/// constructing the [`Terminal`].
///
/// ## Panics
///
/// Does not intentionally panic. A poisoned internal event-source lock inside
/// lower-level input code would still panic if encountered during later use.
///
/// ## Usage note
///
/// This function installs no panic hook. Teardown state lives inside the
/// returned terminal, so install your own hook if the application needs
/// best-effort restoration after panic. Pair normal exits with [`try_restore`]
/// or [`restore`].
pub fn try_init() -> io::Result<DefaultTerminal> {
    try_init_with_options(
        TerminalOptions {
            viewport: Viewport::Fullscreen,
        },
        ProgramOptions::default(),
    )
}

/// Initialize a terminal with explicit options and panic on failure.
///
/// This is the infallible convenience wrapper around
/// [`try_init_with_options`].
///
/// ## Parameters
///
/// * `options` - widget-library terminal options, including the viewport.
/// * `screen_options` - uncurses screen defaults to apply during screen init.
///
/// ## Returns
///
/// A ready-to-draw [`DefaultTerminal`] configured with the supplied options.
///
/// ## Panics
///
/// Panics with `failed to initialize terminal` if [`try_init_with_options`]
/// returns an error.
///
/// ## Usage note
///
/// Use the fallible form in libraries or in applications that need custom error
/// reporting. Pair a successful call with [`restore`] or [`try_restore`].
pub fn init_with_options(
    options: TerminalOptions,
    screen_options: ProgramOptions,
) -> DefaultTerminal {
    try_init_with_options(options, screen_options).expect("failed to initialize terminal")
}

/// Initialize a terminal with explicit terminal and screen options.
///
/// Setup creates an [`UncursesBackend`] over process stdio, calls
/// [`UncursesBackend::init_with`] with `screen_options`, conditionally enters
/// the alternate screen, hides the cursor, records `options.viewport` in the
/// backend, and finally calls [`Terminal::with_options`].
///
/// ## Parameters
///
/// * `options` - terminal options from the widget library. The viewport is
///   cloned before constructing the [`Terminal`] so the backend can mirror the
///   same viewport behavior.
/// * `screen_options` - screen defaults controlling bracketed paste and mouse
///   tracking.
///
/// ## Returns
///
/// A ready-to-draw [`DefaultTerminal`].
///
/// ## Errors
///
/// Returns any error from opening stdio, screen initialization, alternate
/// screen entry, cursor hiding, or [`Terminal::with_options`].
///
/// ## Panics
///
/// Does not intentionally panic.
///
/// ## Usage note
///
/// [`Viewport::Inline`] stays on the main screen; all other viewports enter the
/// alternate screen before the terminal is returned. Always restore with
/// [`try_restore`] or [`restore`] after a successful setup.
pub fn try_init_with_options(
    options: TerminalOptions,
    screen_options: ProgramOptions,
) -> io::Result<DefaultTerminal> {
    let viewport = options.viewport.clone();
    let mut backend = UncursesBackend::stdio()?;
    backend.init_with(screen_options)?;
    if !matches!(viewport, Viewport::Inline(_)) {
        backend.program_mut().enter_alt_screen()?;
    }
    backend.program_mut().hide_cursor()?;
    backend.set_viewport(viewport);
    Terminal::with_options(backend, options)
}

/// Restore a terminal built by the setup helpers, logging any error.
///
/// This convenience wrapper calls [`try_restore`] and writes a diagnostic to
/// standard error if restoration fails. It is intended for shutdown paths where
/// there is no useful way to return an error.
///
/// ## Parameters
///
/// * `terminal` - a terminal returned by this module's setup helpers.
///
/// ## Panics
///
/// Does not intentionally panic.
///
/// ## Usage note
///
/// Use [`try_restore`] when the caller can propagate or inspect restoration
/// errors.
pub fn restore(terminal: &mut DefaultTerminal) {
    if let Err(e) = try_restore(terminal) {
        eprintln!("failed to restore terminal: {e}");
    }
}

/// Restore a terminal built by the setup helpers.
///
/// This calls [`UncursesBackend::restore`] on the terminal's backend. The
/// backend delegates to the wrapped screen's teardown path, which resets staged
/// modes, restores cursor and screen state, flushes pending output, and leaves
/// raw mode.
///
/// ## Parameters
///
/// * `terminal` - a mutable [`DefaultTerminal`] returned by [`init`],
///   [`try_init`], [`init_with_options`], or [`try_init_with_options`].
///
/// ## Returns
///
/// `Ok(())` once teardown has completed.
///
/// ## Errors
///
/// Returns any error from screen teardown, flushing, or restoring the terminal
/// mode.
///
/// ## Panics
///
/// Does not intentionally panic.
///
/// ## Usage note
///
/// Treat this as the single teardown entry point for terminals initialized by
/// this module; do not independently undo individual modes first.
pub fn try_restore(terminal: &mut DefaultTerminal) -> io::Result<()> {
    terminal.backend_mut().restore()
}

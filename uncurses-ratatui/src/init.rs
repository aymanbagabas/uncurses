//! Batteries-included setup/teardown mirroring `ratatui::init` and
//! friends.
//!
//! The [`UncursesBackend`] constructors are deliberately explicit (they
//! do not touch raw mode or the alternate screen). This module layers the
//! usual setup on top and hands back a ready-to-use ratatui [`Terminal`]:
//!
//! * [`init`] / [`try_init`] — full-screen: raw mode, alternate screen,
//!   hidden cursor.
//! * [`init_with_options`] / [`try_init_with_options`] — raw mode and a
//!   hidden cursor with a caller-chosen [`Viewport`]; the alternate
//!   screen is entered for every viewport except [`Viewport::Inline`]
//!   (inline stays on the main screen to preserve scrollback).
//! * [`restore`] / [`try_restore`] — undo the above.

use std::io;

use ratatui::{Terminal, TerminalOptions, Viewport};
use uncurses::terminal::{Stdin, Stdout};

use crate::UncursesBackend;

/// A ratatui [`Terminal`] over the process stdio, the type the `init`
/// functions return.
pub type DefaultTerminal = Terminal<UncursesBackend<Stdin, Stdout>>;

/// Initialize a full-screen terminal over the process stdio, panicking
/// with a message if setup fails. See [`try_init`] for the fallible form.
pub fn init() -> DefaultTerminal {
    try_init().expect("failed to initialize terminal")
}

/// Initialize a full-screen terminal over the process stdio: enter raw
/// mode, switch to the alternate screen, and hide the cursor. Pair with
/// [`restore`] on exit.
///
/// Installs no panic hook: the teardown state is owned by the returned
/// terminal rather than global, so a hook could not reach it. Install
/// your own (capturing the terminal, or emitting a best-effort reset) if
/// you need panic safety.
pub fn try_init() -> io::Result<DefaultTerminal> {
    try_init_with_options(TerminalOptions {
        viewport: Viewport::Fullscreen,
    })
}

/// Initialize a terminal with the given options, panicking with a message
/// if setup fails. See [`try_init_with_options`] for the fallible form.
pub fn init_with_options(options: TerminalOptions) -> DefaultTerminal {
    try_init_with_options(options).expect("failed to initialize terminal")
}

/// Initialize a terminal with the given viewport: enter raw mode and hide
/// the cursor, and switch to the alternate screen for every viewport
/// except [`Viewport::Inline`]. Fullscreen and fixed viewports render on
/// the alternate screen; an inline viewport stays on the main screen so
/// the surrounding shell output and scrollback are preserved.
pub fn try_init_with_options(options: TerminalOptions) -> io::Result<DefaultTerminal> {
    let viewport = options.viewport.clone();
    let mut backend = UncursesBackend::stdio()?;
    backend.init()?;
    backend
        .screen_mut()
        .set_alt_screen(!matches!(viewport, Viewport::Inline(_)));
    backend.screen_mut().set_cursor_visible(false);
    backend.set_viewport(viewport);
    Terminal::with_options(backend, options)
}

/// Restore a terminal built by the `init` functions, logging any error.
/// See [`try_restore`] for the fallible form.
pub fn restore(terminal: &mut DefaultTerminal) {
    if let Err(e) = try_restore(terminal) {
        eprintln!("failed to restore terminal: {e}");
    }
}

/// Restore a terminal: reset the screen (leave the alternate screen, show
/// the cursor, clear modes), flush, and leave raw mode. The single
/// teardown entry point — do not undo the modes individually.
pub fn try_restore(terminal: &mut DefaultTerminal) -> io::Result<()> {
    terminal.backend_mut().restore()
}

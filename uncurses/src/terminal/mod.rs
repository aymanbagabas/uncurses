//! Terminal handles, raw-mode state, and environment helpers.
//!
//! This module provides owned and standard terminal handles, raw-mode save and restore functions, window-size queries, and tty opening utilities.
//! Reach for it when wiring a [`crate::canvas::Canvas`] or [`crate::event::EventSource`] to a real terminal, or when tests need a controlled [`Env`] snapshot.
//!
//! For raw byte-level control without a renderer, drive a [`Terminal`]
//! directly: enter raw mode, query the window size, and restore the prior
//! state on the way out.
//!
//! ```no_run
//! use uncurses::terminal::Terminal;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut term = Terminal::stdio();
//! let _prior = term.make_raw()?;        // enter raw mode
//! let size = term.get_window_size()?;   // columns and rows (and pixels, if known)
//! let _ = (size.col, size.row);
//! term.restore()?;                      // put the terminal back
//! # Ok(())
//! # }
//! ```

pub mod env;
mod handle;
pub mod raw;
pub mod size;
pub mod stdio;
pub mod tty;

pub use env::Env;
pub use handle::Terminal;
pub use raw::*;
pub use size::*;
pub use stdio::{Stderr, Stdin, Stdout, stderr, stdin, stdout};
pub use tty::*;

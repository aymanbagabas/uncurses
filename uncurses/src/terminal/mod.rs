//! Terminal handles, raw-mode state, window-size queries, and tty helpers.
//!
//! The terminal module provides the building blocks used by renderers and
//! event sources: [`Terminal`] for pairing input and output handles,
//! raw-mode save/restore helpers, direct stdio handles, controlling-terminal
//! opening, and environment snapshots.
//!
//! ## The `Terminal<I, O>` handle
//!
//! [`Terminal`] owns an input half, an output half, an [`Env`] snapshot, and an
//! optional saved raw-mode [`State`]. It implements [`std::io::Read`] by
//! reading from the input half and [`std::io::Write`] by writing to the output
//! half. Use [`Terminal::stdio`] for inherited stdin/stdout, or
//! [`Terminal::open`] to talk directly to the controlling terminal when stdio
//! may be redirected.
//!
//! ## Raw-mode lifecycle
//!
//! Raw mode is an explicit save/apply/restore flow. [`Terminal::make_raw`]
//! calls [`make_raw_mode`], stores the returned [`State`] inside the
//! `Terminal`, and returns a clone of that state. [`Terminal::restore`] applies
//! the stored state with [`set_state`] and clears it. The free functions expose
//! the same lifecycle for callers that manage handles and state themselves.
//!
//! ```text
//! normal terminal state
//!         │
//!         │ make_raw() -> State
//!         ▼
//! raw terminal state ────── use terminal ──────┐
//!         │                                    │
//!         └──────── restore()/set_state(State) ◀
//! ```
//!
//! There is no paired enable/disable function in this API; keep the returned
//! [`State`] or the [`Terminal`] that cached it, and restore explicitly.
//!
//! ## Window size
//!
//! [`get_window_size`] queries the operating system for cell dimensions and,
//! where available, pixel dimensions. [`Terminal::get_window_size`] applies the
//! platform-specific handle selection: Unix tries output first, then input;
//! Windows queries the output console screen buffer.
//!
//! ## Stdio and controlling-terminal handles
//!
//! [`stdin`], [`stdout`], and [`stderr`] wrap the inherited process streams as
//! cheap, copyable, unbuffered handles. [`open_tty`] opens the controlling
//! terminal directly: Unix uses `/dev/tty` for both halves, and Windows uses
//! `CONIN$` for input and `CONOUT$` for output. The direct tty path is useful
//! for applications whose stdin or stdout may be a pipe but which still need a
//! real terminal.
//!
//! ```rust,ignore
//! use uncurses::terminal::Terminal;
//!
//! let mut term = Terminal::stdio();
//! let saved = term.make_raw()?;
//! let size = term.get_window_size()?;
//! let _ = (saved, size.col, size.row);
//! term.restore()?;
//! # Ok::<(), std::io::Error>(())
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

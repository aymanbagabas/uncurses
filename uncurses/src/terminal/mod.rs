//! Terminal handles, raw-mode state, and environment helpers.
//!
//! This module provides owned and standard terminal handles, raw-mode save and restore functions, window-size queries, and tty opening utilities.
//! Reach for it when wiring a [`crate::canvas::Canvas`] or [`crate::event::EventSource`] to a real terminal, or when tests need a controlled [`Env`] snapshot.

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

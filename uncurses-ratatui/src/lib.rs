//! Integration adapter that lets a `ratatui::Terminal` render through a
//! [`uncurses::screen::Screen`].
//!
//! [`UncursesBackend`] owns the whole terminal stack — the
//! [`Terminal`](uncurses::Terminal) handle, the [`Screen`](uncurses::screen::Screen),
//! and a shared [`EventSource`](uncurses::event::EventSource) — so a
//! single value drives rendering, input, and the raw-mode lifecycle.

mod backend;
mod convert;
mod init;

pub use backend::{OutputHandle, UncursesBackend};
pub use convert::{to_uncurses_color, to_uncurses_style};
pub use init::{
    DefaultTerminal, init, init_with_options, restore, try_init, try_init_with_options, try_restore,
};

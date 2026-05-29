//! Integration adapter that lets a `ratatui::Terminal` render through a
//! [`uncurses::screen::Screen`].
//!
//! The adapter is render-only. Drive your event loop directly via
//! [`uncurses::event`].

mod backend;
mod convert;

pub use backend::UncursesBackend;
pub use convert::{to_uncurses_color, to_uncurses_style};

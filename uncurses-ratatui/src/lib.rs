//! A [`ratatui`] `Backend` that renders through the high-level
//! [`uncurses::screen::Screen`] facade. Write your UI with ratatui widgets
//! and let uncurses diff frames and ship the minimal byte changes.
//!
//! [`UncursesBackend`] wraps a [`Screen`](uncurses::screen::Screen), which
//! bundles the whole terminal stack: the
//! [`Terminal`](uncurses::terminal::Terminal) handle, the
//! [`Canvas`](uncurses::canvas::Canvas), and the event source. A single
//! value drives rendering, input, and the raw-mode lifecycle, and you still
//! run your own event loop through the same backend.
//!
//! # Quick start
//!
//! The [`init`] / [`restore`] helpers mirror ratatui's own setup
//! functions: they enter raw mode, hide the cursor, pick the viewport, and
//! hand back a ready-to-go ratatui terminal.
//!
//! ```no_run
//! use ratatui::widgets::Paragraph;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut terminal = uncurses_ratatui::try_init()?;
//! terminal.draw(|frame| {
//!     frame.render_widget(Paragraph::new("from ratatui, via uncurses"), frame.area());
//! })?;
//! // The backend owns the event source; read input through it, then:
//! uncurses_ratatui::restore(&mut terminal);
//! # Ok(())
//! # }
//! ```
//!
//! See the crate README and the `ratatui_*` examples for input handling,
//! inline viewports, and (with the `async` feature) an event stream.
//!
//! [`ratatui`]: https://docs.rs/ratatui/latest/ratatui/

mod backend;
mod convert;
mod init;

pub use backend::{OutputHandle, UncursesBackend};
pub use convert::{to_uncurses_color, to_uncurses_style};
pub use init::{
    DefaultTerminal, init, init_with_options, restore, try_init, try_init_with_options, try_restore,
};
#[doc(no_inline)]
pub use uncurses::screen::{MousePreference, ScreenOptions};

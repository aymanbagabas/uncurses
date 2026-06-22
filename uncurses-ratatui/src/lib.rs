//! Backend integration between the widget library and
//! [`uncurses::screen::Screen`].
//!
//! ## What this backend is
//!
//! [`UncursesBackend`] implements [`ratatui::backend::Backend`] by wrapping a
//! single high-level [`Screen`](uncurses::screen::Screen). The screen owns the
//! terminal handle, its diffing renderer, and the input source. The backend's job is to adapt
//! frame drawing, cursor operations, clearing, size queries, and event access
//! to that one screen.
//!
//! ## Rendering path
//!
//! A frame is rendered into the widget library's buffer first. During
//! [`Backend::draw`](ratatui::backend::Backend::draw), every visible buffer
//! cell is converted into an uncurses cell and staged into the screen's
//! buffer; no I/O happens yet.
//! [`Backend::flush`](ratatui::backend::Backend::flush) then calls
//! [`Screen::render`](uncurses::screen::Screen::render), which diffs the
//! staged frame and writes the minimal escape bytes.
//!
//! ```text
//! ┌──────────────────────┐
//! │ widgets render Frame │
//! └──────────┬───────────┘
//!            │ ratatui::buffer::Cell values
//!            ▼
//! ┌──────────────────────┐
//! │   UncursesBackend    │
//! │ draw: Cell → Cell    │
//! └──────────┬───────────┘
//!            │ set_cell (stage only)
//!            ▼
//! ┌──────────────────────┐
//! │        Screen        │
//! │   renderer diff bytes │
//! └──────────┬───────────┘
//!            │ flush → Screen::render
//!            ▼
//!        terminal
//! ```
//!
//! ## Setup and restore helpers
//!
//! Use [`try_init`] / [`init`] for the standard fullscreen session over
//! process stdio. Use [`try_init_with_options`] / [`init_with_options`] when
//! you need explicit [`ratatui::TerminalOptions`] or [`ScreenOptions`]. Pair
//! those helpers with [`try_restore`] or [`restore`] on exit.
//!
//! The helpers enter raw mode, apply screen options, hide the cursor, and set
//! up the requested viewport. They enter the alternate screen for fullscreen
//! and fixed viewports; inline viewports stay on the main screen so scrollback
//! around the application is preserved.
//!
//! ```rust,ignore
//! use ratatui::widgets::Paragraph;
//!
//! fn main() -> std::io::Result<()> {
//!     let mut terminal = uncurses_ratatui::try_init()?;
//!     terminal.draw(|frame| {
//!         frame.render_widget(Paragraph::new("drawn through uncurses"), frame.area());
//!     })?;
//!     uncurses_ratatui::try_restore(&mut terminal)
//! }
//! ```
//!
//! ## Viewports
//!
//! [`ratatui::Viewport::Fullscreen`] and [`ratatui::Viewport::Fixed`] are
//! rendered on the alternate screen by the setup helpers. For
//! [`ratatui::Viewport::Inline`], the backend keeps an inline origin in the
//! main screen, resizes the screen buffer to the inline height, and translates
//! absolute frame rows into that buffer before staging cells.
//!
//! ## Reading input through the backend
//!
//! The backend owns the same input source as the wrapped screen. Synchronous
//! event loops can call [`UncursesBackend::poll_event`],
//! [`UncursesBackend::try_read_event`], or [`UncursesBackend::read_event`] on
//! `terminal.backend_mut()`. These methods also let the screen observe
//! capability replies that arrive on the input stream.
//!
//! With the `async` feature, use [`UncursesBackend::screen_mut`] and the
//! screen's `events()` stream when an asynchronous loop is more convenient.
//!
//! ## Manual setup
//!
//! The constructors are inert: they do not enter raw mode, choose a viewport,
//! enter the alternate screen, or hide the cursor. Use them when you need a
//! prebuilt [`Screen`](uncurses::screen::Screen), a controlling terminal
//! instead of stdio, or custom setup ordering.
//!
//! ```rust,ignore
//! use uncurses_ratatui::{ScreenOptions, UncursesBackend};
//!
//! fn main() -> std::io::Result<()> {
//!     let mut backend = UncursesBackend::stdio()?;
//!     backend.init_with(ScreenOptions::default())?;
//!     // Build ratatui::Terminal with `backend`, then restore the backend
//!     // when the session ends.
//!     Ok(())
//! }
//! ```

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

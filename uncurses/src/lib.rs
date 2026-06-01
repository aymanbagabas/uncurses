//! uncurses — A Rust terminal rendering library.
//!
//! Provides a complete, efficient API for building terminal user interfaces,
//! including cell buffers, ANSI escape sequence generation/parsing, screen
//! diffing with scroll optimization, input event handling, and terminal
//! state management.
//!
//! # Architecture
//!
//! ```text
//! Terminal → Screen → Buffer/Window (cell model)
//!                              + internal cell-diff to ANSI
//! ```
//!
//! # Quick Start
//!
//! ```rust
//! use std::io::Write;
//! use uncurses::screen::Screen;
//! use uncurses::style::Style;
//! use uncurses::color::{Color, BasicColor};
//! use uncurses::text::WrapMode;
//!
//! let mut screen = Screen::new(std::io::stdout()).with_size(80, 24);
//! let style = Style::EMPTY
//!     .bold()
//!     .with_fg(Color::Basic(BasicColor::Green));
//! screen.set_str_with((0, 0), "Hello, terminal!", WrapMode::Truncate, style);
//!
//! screen.render().unwrap();
//! screen.flush().unwrap();
//! ```

//! # Output buffering and flushing
//!
//! [`screen::Screen`] owns the writer and stages every byte it emits
//! into an internal buffer. Nothing reaches the underlying writer until
//! [`std::io::Write::flush`] is called on the screen, which gives
//! callers control over exactly when a frame plus its mode changes hit
//! the wire.
//!
//! ```ignore
//! use std::io::{self, Write};
//!
//! let mut screen = uncurses::screen::Screen::new(io::stdout()).with_size(80, 24);
//! // … screen.render()?; screen.set_alt_screen(true)?; …
//! screen.flush()?; // explicit: nothing reaches the terminal until here
//! # Ok::<_, std::io::Error>(())
//! ```

pub mod ansi;
pub mod buffer;
pub mod cell;
pub mod color;
pub mod event;
pub mod layout;
#[cfg(feature = "bench")]
pub mod renderer;
#[cfg(not(feature = "bench"))]
pub(crate) mod renderer;
pub mod screen;
pub mod style;
pub mod terminal;
pub mod text;

#[cfg(debug_assertions)]
mod trace;

pub use buffer::{Bounded, Surface, SurfaceMut};
pub use layout::{Position, Rect};

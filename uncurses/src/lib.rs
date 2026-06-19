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
//! Terminal → Canvas → Buffer/Window (cell model)
//!                              + internal cell-diff to ANSI
//! ```
//!
//! # Quick Start
//!
//! ```rust
//! use std::io::Write;
//! use uncurses::canvas::Canvas;
//! use uncurses::style::Style;
//! use uncurses::color::{Color, BasicColor};
//! use uncurses::terminal::stdout;
//!
//! let mut screen = Canvas::new(stdout(), (80, 24));
//! let style = Style::default()
//!     .bold()
//!     .fg(Color::Basic(BasicColor::Green));
//! screen.set_str((0, 0), "Hello, terminal!", style);
//!
//! screen.render();
//! screen.flush().unwrap();
//! ```

//! # Output buffering and flushing
//!
//! [`canvas::Canvas`] owns the writer and stages every byte it emits
//! into an internal buffer. Nothing reaches the underlying writer until
//! [`std::io::Write::flush`] is called on the screen, which gives
//! callers control over exactly when a frame plus its mode changes hit
//! the wire.
//!
//! ```ignore
//! use std::io::Write;
//! use uncurses::terminal::stdout;
//!
//! let mut screen = uncurses::canvas::Canvas::new(stdout(), (80, 24));
//! // … screen.render(); screen.set_alt_screen(true); …
//! screen.flush()?; // explicit: nothing reaches the terminal until here
//! # Ok::<_, std::io::Error>(())
//! ```

#[cfg(not(any(feature = "icu", feature = "unicode-rs")))]
compile_error!(
    "uncurses requires one of the `icu` or `unicode-rs` features to be enabled (the default)"
);

pub mod ansi;
pub mod buffer;
pub mod canvas;
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

//! `uncurses` is a low-level terminal library for building terminal user
//! interfaces. It hands you the pieces (a cell grid with a diffing
//! renderer, a typed input decoder, ANSI escape helpers, and a raw-mode
//! terminal handle) and stays out of the way: you own the event loop and
//! decide when bytes hit the wire. There is no terminfo database and no
//! widget tree.
//!
//! # Where to start
//!
//! Two layers cover most needs. Pick the one that fits how much control
//! you want.
//!
//! - **[`screen::Screen`]** is the high-level facade. It owns a terminal,
//!   a [`canvas::Canvas`], and an [`event::EventSource`], and manages raw
//!   mode, capability detection, sane default modes, and teardown. Reach
//!   for it to get an interactive app running quickly. See the
//!   [`screen`] module docs for the full lifecycle.
//! - **[`canvas::Canvas`]** is the cell grid and diffing renderer on its
//!   own. Drive it directly when you want to manage raw mode, input, and
//!   terminal modes yourself. It works in both inline and fullscreen
//!   layouts.
//!
//! # Quick start (high-level)
//!
//! ```no_run
//! use uncurses::buffer::SurfaceMut;
//! use uncurses::color::BasicColor;
//! use uncurses::screen::Screen;
//! use uncurses::style::Style;
//! use uncurses::text::TextSurface;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut screen = Screen::stdio()?;
//! screen.init()?; // raw mode + capability detection
//!
//! let style = Style::default().bold().fg(BasicColor::Green);
//! screen.set_str((0, 0), "Hello, terminal!", style);
//! screen.present()?; // stage the diff and flush it
//!
//! screen.finish() // tear down modes and restore the terminal
//! # }
//! ```
//!
//! # Quick start (low-level)
//!
//! Build a [`canvas::Canvas`] over any writer and drive it yourself:
//!
//! ```rust
//! use std::io::Write;
//! use uncurses::canvas::Canvas;
//! use uncurses::style::Style;
//! use uncurses::color::{Color, BasicColor};
//! use uncurses::terminal::stdout;
//! use uncurses::text::TextSurface;
//!
//! let mut canvas = Canvas::new(stdout(), (80, 24));
//! let style = Style::default()
//!     .bold()
//!     .fg(Color::Basic(BasicColor::Green));
//! canvas.set_str((0, 0), "Hello, terminal!", style);
//!
//! canvas.render();
//! canvas.flush().unwrap();
//! ```
//!
//! # The module map
//!
//! | Module | What lives there |
//! | --- | --- |
//! | [`screen`] | The self-managing [`Screen`](screen::Screen) facade. |
//! | [`canvas`] | The [`Canvas`](canvas::Canvas) cell grid and diffing renderer. |
//! | [`buffer`] | Cell-grid storage and the [`Surface`](buffer::Surface) / [`SurfaceMut`](buffer::SurfaceMut) traits every drawable shares. |
//! | [`text`] | Text shaping, width measurement, and the [`TextSurface`](text::TextSurface) painting trait that adds `set_str` to any surface. |
//! | [`style`] | [`Style`](style::Style), colors, attributes, and SGR plus hyperlink (OSC 8) encoding. |
//! | [`color`] | Color types and capability [`Profile`](color::Profile)s with automatic downsampling. |
//! | [`event`] | The [`EventSource`](event::EventSource) decoder, typed [`Event`](event::Event) values, and (with the `async` feature) an `EventStream`. |
//! | [`ansi`] | Raw escape-sequence encoders and parsers for the cursor, modes, colors, queries, and the long tail of terminal control. |
//! | [`terminal`] | The [`Terminal`](terminal::Terminal) handle, raw-mode lifecycle, window-size queries, and environment snapshot. |
//! | [`cell`] | The [`Cell`](cell::Cell) value type and grapheme segmentation. |
//! | [`layout`] | [`Position`](layout::Position), [`Size`](layout::Size), and [`Rect`](layout::Rect) geometry. |
//!
//! # Output buffering and flushing
//!
//! Drawing is infallible. Both [`Canvas`](canvas::Canvas) and
//! [`Screen`](screen::Screen) stage every byte they emit into an internal
//! buffer; nothing reaches the underlying writer until a flush. The
//! [`Canvas`](canvas::Canvas) flushes when you call
//! [`std::io::Write::flush`] (or [`present`](canvas::Canvas::present),
//! which renders and flushes). The one place I/O can fail is the flush, so
//! the hot path stays simple and the error handling stays honest.
//!
//! ```ignore
//! use std::io::Write;
//! use uncurses::terminal::stdout;
//!
//! let mut canvas = uncurses::canvas::Canvas::new(stdout(), (80, 24));
//! // … canvas.render(); canvas.set_alt_screen(true); …
//! canvas.flush()?; // explicit: nothing reaches the terminal until here
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

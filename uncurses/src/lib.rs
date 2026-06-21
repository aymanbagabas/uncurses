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
//! - **[`screen::Screen`]** is the high-level facade and the home of the
//!   diffing renderer. It owns a terminal and an [`event::EventSource`],
//!   tracks the live terminal across frames, and emits only the cells that
//!   changed. It also manages raw mode, capability detection, sane default
//!   modes, and teardown. Reach for it to drive an interactive app, in
//!   either inline or fullscreen layout. See the [`screen`] module docs for
//!   the full lifecycle.
//! - **[`buffer::TextBuffer`]** (and any [`buffer::Surface`]) is the
//!   stateless route. Paint a full frame into an in-memory grid and
//!   serialize it to escape bytes with the [`text::Encode`] trait. There is
//!   no renderer and no terminal session, which makes it the tool for
//!   one-shot frames, snapshot tests, transcripts, and append-style output.
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
//! Paint a [`buffer::TextBuffer`] and serialize it yourself, with no
//! terminal involved:
//!
//! ```rust
//! use uncurses::buffer::TextBuffer;
//! use uncurses::color::{Color, BasicColor};
//! use uncurses::style::Style;
//! use uncurses::text::{Encode, TextSurface};
//!
//! let mut frame = TextBuffer::new(80, 24);
//! let style = Style::default()
//!     .bold()
//!     .fg(Color::Basic(BasicColor::Green));
//! frame.set_str((0, 0), "Hello, terminal!", style);
//!
//! // Serialize the painted grid to escape bytes you can write anywhere.
//! let bytes = frame.display().to_string();
//! assert!(bytes.contains("Hello, terminal!"));
//! ```
//!
//! # The module map
//!
//! | Module | What lives there |
//! | --- | --- |
//! | [`screen`] | The self-managing [`Screen`](screen::Screen) facade and its diffing renderer. |
//! | [`buffer`] | Cell-grid storage ([`Buffer`](buffer::Buffer), [`TextBuffer`](buffer::TextBuffer), [`Window`](buffer::Window)) and the [`Surface`](buffer::Surface) / [`SurfaceMut`](buffer::SurfaceMut) traits every drawable shares. |
//! | [`text`] | Text shaping, width measurement, the [`TextSurface`](text::TextSurface) painting trait that adds `set_str` to any surface, and the [`Encode`](text::Encode) trait that serializes a surface to escapes. |
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
//! Drawing is infallible. [`Screen`](screen::Screen) stages every byte it
//! emits into an internal buffer; nothing reaches the underlying writer
//! until a flush. It flushes when you call [`present`](screen::Screen::present)
//! (which renders the diff and flushes) or [`std::io::Write::flush`]. The one
//! place I/O can fail is the flush, so the hot path stays simple and the
//! error handling stays honest. A stateless [`TextBuffer`](buffer::TextBuffer)
//! has no writer of its own: [`encode`](text::Encode::encode) hands you the
//! bytes and you decide where they go.

#[cfg(not(any(feature = "icu", feature = "unicode-rs")))]
compile_error!(
    "uncurses requires one of the `icu` or `unicode-rs` features to be enabled (the default)"
);

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

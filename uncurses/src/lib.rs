//! `uncurses` is a Rust library for building terminal user interfaces. It
//! provides a direct, framework-free way to draw to the terminal and read
//! input, giving you control over every cell and your own event loop, whether
//! you run inline, take over the full screen, mix the two, or leave the
//! console unmanaged and just shape your output. It hands you the pieces (a
//! cell grid with a diffing renderer, a typed input decoder, ANSI escape
//! helpers, and a raw-mode terminal handle) and decides nothing for you.
//! There is no terminfo database and no widget tree.
//!
//! # Where to start
//!
//! Three routes cover most needs. Pick the one that fits your use case.
//!
//! - **[`program::Program`]** is the interactive facade. It owns a terminal,
//!   an [`event::EventSource`], and a `Screen` to draw with. It manages raw
//!   mode, terminal modes, capability tracking, and teardown, and hands you
//!   the screen through [`screen`](program::Program::screen) /
//!   [`screen_mut`](program::Program::screen_mut). Reach for it to drive an
//!   interactive app, inline or fullscreen. See the [`program`] module docs
//!   for the full lifecycle.
//! - **[`screen::Screen`]** is the diffing renderer on its own: a cell grid, a
//!   renderer, and any [`Write`](std::io::Write). You paint cells and call
//!   [`render`](screen::Screen::render); it emits only what changed. It reads
//!   no input and touches no terminal mode, so it stands alone for
//!   output-only programs, tests, and offscreen rendering.
//! - **[`buffer::TextBuffer`]** (and any [`buffer::Surface`]) is the
//!   stateless route. Paint a full frame into an in-memory grid and
//!   serialize it to escape bytes with the [`text::Encode`] trait. There is
//!   no renderer and no terminal session, which makes it the tool for
//!   one-shot frames, snapshot tests, transcripts, and append-style output.
//!
//! # Quick start with `Program`
//!
//! ```no_run
//! use uncurses::color::Color;
//! use uncurses::program::Program;
//! use uncurses::style::Style;
//! use uncurses::text::TextSurface;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut program = Program::stdio()?;
//! program.init()?; // raw mode
//!
//! let style = Style::default().bold().fg(Color::Green);
//! let screen = program.screen_mut();
//! screen.set_str((0, 0), "Hello, terminal!", style);
//! screen.render()?; // stage the diff and flush it
//!
//! program.finish() // tear down modes and restore the terminal
//! # }
//! ```
//!
//! # Quick start with `TextBuffer`
//!
//! Paint a [`buffer::TextBuffer`] and serialize it yourself, with no
//! terminal involved:
//!
//! ```rust
//! use uncurses::buffer::TextBuffer;
//! use uncurses::color::Color;
//! use uncurses::style::Style;
//! use uncurses::text::{Encode, TextSurface};
//!
//! let mut frame = TextBuffer::new(80, 24);
//! let style = Style::default()
//!     .bold()
//!     .fg(Color::Green);
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
//! | [`cell`] | The [`Cell`](cell::Cell) value type. |
//! | [`unicode`] | Grapheme-cluster segmentation and other Unicode text primitives. |
//! | [`layout`] | [`Position`](layout::Position), [`Size`](layout::Size), and [`Rect`](layout::Rect) geometry. |
//!
//! # Output buffering and flushing
//!
//! Painting is infallible. Drawing cells with
//! [`set_str`](text::TextSurface::set_str),
//! [`set_cell`](screen::Screen::set_cell), and friends only updates an
//! in-memory frame; nothing is written until you call
//! [`render`](screen::Screen::render), which diffs that frame against the
//! terminal and writes just the changed cells.
//!
//! Mode changes are applied immediately. Entering the alternate screen,
//! hiding the cursor, enabling mouse reporting, setting the title, and similar
//! switches write their escape sequence on the spot. A stateless
//! [`TextBuffer`](buffer::TextBuffer) has no writer of its own:
//! [`encode`](text::Encode::encode) hands you the bytes and you decide where
//! they go.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(uncurses_bench, feature(test))]

#[cfg(not(any(feature = "icu", feature = "unicode-rs")))]
compile_error!(
    "uncurses requires one of the `icu` or `unicode-rs` features to be enabled (the default)"
);

/// The `libc` crate this was built against, re-exported so the platform types
/// in the public API are nameable without depending on `libc` directly and
/// matching its major version by hand. [`terminal::State`] exposes
/// `libc::termios` values.
#[cfg(unix)]
pub use libc;

pub mod ansi;
pub mod buffer;
pub mod cell;
pub mod color;
pub mod event;
pub mod layout;
pub mod program;
pub mod screen;
pub mod style;
pub mod terminal;
pub mod text;
pub mod unicode;

pub(crate) mod renderer;

#[cfg(all(test, unix, not(target_os = "l4re")))]
mod testutil;

#[cfg(debug_assertions)]
mod trace;

# uncurses

A terminal rendering library for Rust that hands you the building blocks
and then gets out of the way.

There is no terminfo database here, no widget tree, no hidden global state,
and no framework to wrestle. uncurses assumes a modern, VT100/xterm-style
terminal and talks to it directly. You own the event loop. You decide when
bytes hit the wire. The library just makes the bytes correct and minimal.

## The mental model

A terminal UI is, at heart, three jobs:

1. **Decide what the screen should look like** — a grid of cells, each with
   a character and a style.
2. **Get that picture onto the terminal** — without redrawing everything
   every frame.
3. **React to what the user does** — keys, mouse, paste, resize.

uncurses gives you one type for each job, and one type that bundles all
three when you just want to ship an app.

| You want to... | Reach for |
| --- | --- |
| Ship an interactive app fast | [`Screen`](#screen-the-easy-button) |
| Render cells to *anything* that's `Write` | [`Canvas`](#canvas-the-renderer) |
| Turn raw bytes into typed events | [`EventSource`](#eventsource-the-input) |
| Enter raw mode, ask the window size | [`Terminal`](#terminal-the-device) |
| Emit a specific escape sequence | [`ansi`](#the-rest-of-the-map) |

Drawing is **infallible**. Every draw call writes into an in-memory buffer
and returns nothing to check. The single place I/O can fail is the
*flush*, when that buffer goes to the terminal. The hot path stays simple;
the error handling stays honest.

## `Screen`: the easy button

`Screen` owns a terminal, a renderer, and an input source, and manages raw
mode, capability detection, sensible default modes, and teardown for you.
It is the fastest way from `cargo new` to a running TUI.

```rust,no_run
use uncurses::buffer::SurfaceMut;
use uncurses::color::BasicColor;
use uncurses::event::{Event, Key};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?;             // raw mode + capability detection
    screen.enter_alt_screen()?; // take over the whole window
    screen.hide_cursor()?;

    let quit: [Key; 2] = ["q", "esc"].map(|s| s.parse().unwrap());
    loop {
        screen.clear();
        let style = Style::default().bold().fg(BasicColor::Green);
        screen.set_str((0, 0), "Hello, terminal! Press q to quit.", style);
        screen.present()?;       // render the diff and flush it

        match screen.read_event()? {
            Event::KeyPress(ref k) if quit.contains(k) => break,
            Event::Resize(ws) => screen.resize((ws.col, ws.row)),
            _ => {}
        }
    }

    screen.finish() // restore the terminal: one call, always
}
```

The lifecycle is explicit and there is no `Drop` magic:

- [`Screen::stdio`] / [`Screen::open`] build it; [`init`](Screen::init) (or
  [`init_with`](Screen::init_with) for [`ScreenOptions`]) begins the
  session.
- [`finish`](Screen::finish) consumes the screen and restores the
  terminal. [`pause`](Screen::pause) / [`resume`](Screen::resume) hand the
  terminal back temporarily (shell out, then come back), and
  [`suspend`](Screen::suspend) does that plus `SIGTSTP`.

Read events with [`read_event`](Screen::read_event) (blocking),
[`poll_event`](Screen::poll_event) + [`try_read_event`](Screen::try_read_event)
(non-blocking), or, with the `async` feature,
[`events`](Screen::events) (a `Stream` you `.await`).

## `Canvas`: the renderer

`Canvas<W>` is the cell grid and the diffing renderer on their own, over
**any** `Write` sink. It has no opinion about input, raw mode, or
lifecycle. Drive it yourself when you want full control, or point it at
something that isn't a terminal at all:

```rust
use std::io::Write;
use uncurses::canvas::Canvas;
use uncurses::color::BasicColor;
use uncurses::style::Style;
use uncurses::text::TextSurface;

// Render into a byte buffer. No terminal required.
let mut canvas: Canvas<Vec<u8>> = Canvas::new(Vec::new(), (40, 1));
canvas.set_str((0, 0), "rendered off-screen", Style::default().fg(BasicColor::Cyan));
canvas.render();
canvas.flush().unwrap();

// `canvas.writer()` now holds the exact bytes the renderer produced.
assert!(!canvas.writer().is_empty());
```

That sink can be a `Vec<u8>`, a pipe, a socket, a snapshot test, or a
transcript recorder. `Screen` always owns input and a terminal lifecycle;
`Canvas` is what you want when you only need pixels-as-cells out.

Both `Canvas` and `Screen` buffer everything they emit and only touch the
writer on flush. `render()` computes the minimal diff and stages the
bytes; `flush()` ships them; [`present`](uncurses::canvas::Canvas::present)
does both.

## `EventSource`: the input

`EventSource<I>` turns the raw byte stream into typed [`Event`] values:
keys, mouse, paste chunks, focus, resize, and terminal query replies. It
handles the gnarly parts — multi-byte escape sequences, ambiguous prefixes,
the `ESC`-key-versus-escape-sequence timeout — so you match on an enum
instead of a byte soup.

```rust,no_run
use uncurses::event::{Event, EventSource};
use uncurses::terminal::Terminal;

let mut term = Terminal::stdio();
term.make_raw()?;
let mut events = EventSource::new(term.input())?;

match events.read()? {
    Event::KeyPress(key) => println!("key: {key:?}\r"),
    Event::Resize(size)  => println!("resized to {}x{}\r", size.col, size.row),
    _ => {}
}
# Ok::<(), std::io::Error>(())
```

Keys compare by their canonical chord, and [`Key`] implements `FromStr`, so
matching a shortcut is plain equality: `key == "ctrl+c".parse().unwrap()`.

## `Terminal`: the device

`Terminal<I, O>` is the thin handle over the tty: raw mode on and off
(remembering the prior state so teardown takes no arguments), the window
size, and a snapshot of the relevant environment. [`Terminal::stdio`] uses
the process stdio; [`Terminal::open`] talks to the controlling terminal
directly (`/dev/tty`, or `CONIN$`/`CONOUT$` on Windows) even when stdio is
redirected.

## The rest of the map

| Module | What lives there |
| --- | --- |
| [`screen`] | The self-managing [`Screen`](screen::Screen) facade. |
| [`canvas`] | The [`Canvas`](canvas::Canvas) cell grid and diffing renderer. |
| [`buffer`] | Cell storage and the [`Surface`](buffer::Surface) / [`SurfaceMut`](buffer::SurfaceMut) traits every drawable shares. |
| [`text`] | Width measurement, grapheme handling, and the [`TextSurface`](text::TextSurface) trait that adds `set_str` to any surface. |
| [`style`] | [`Style`](style::Style), attributes, and SGR plus hyperlink (OSC 8) encoding. |
| [`color`] | Color types and capability [`Profile`](color::Profile)s with automatic downsampling. |
| [`event`] | The [`EventSource`](event::EventSource) decoder, [`Event`](event::Event) values, and (with `async`) an `EventStream`. |
| [`ansi`] | Raw escape encoders and parsers for the cursor, modes, colors, queries, and the long tail of terminal control. |
| [`terminal`] | The [`Terminal`](terminal::Terminal) handle, raw-mode lifecycle, and window-size queries. |
| [`cell`] | The [`Cell`](cell::Cell) value type and grapheme segmentation. |
| [`layout`] | [`Position`](layout::Position), [`Size`](layout::Size), and [`Rect`](layout::Rect) geometry. |

## Styling and color

Build a [`Style`] fluently and hand it to any draw call:

```rust
use uncurses::color::{BasicColor, Color};
use uncurses::style::Style;

let style = Style::default()
    .bold()
    .italic()
    .fg(BasicColor::BrightWhite)
    .bg(Color::Indexed(236));
```

A color setter accepts a `Color`, a `BasicColor`, or `None` to clear it
when reusing a base style. Colors can be built from hex or HSL and read
back the same way:

```rust
use uncurses::color::Color;
use uncurses::style::Style;

let pink = Color::hex("#ff69b4").unwrap(); // -> Color::Rgb(255, 105, 180)
let teal = Color::hsl(180.0, 0.5, 0.4);
let hex = teal.to_hex();                   // "#339999"
let (_h, _s, _l) = pink.to_hsl();

let base = Style::default().bold().fg(pink);
let plain = base.clone().fg(None);         // keep bold, drop the color
```

Colors are 24-bit RGB, 256-indexed, or the 16 basics, and downsample
automatically to whatever the active [`Profile`](color::Profile) allows, so
you write true color once and degrade gracefully on a 16-color terminal.

A [`Style`] is also a value you can render by hand: its `Display` is the
opening SGR sequence (no reset) and an empty `Style` is the reset, so a
style and `Style::default()` work as open/close tokens in plain `write!`
calls. The `styles` example uses exactly that.

## Asking the terminal what it can do

uncurses does not ship a capability database, and it does not guess. If you
need to know something — the background color, the cell size in pixels,
which protocols are supported — you *ask*, by writing a request and reading
the reply back as an ordinary [`Event`]. The Primary Device Attributes
reply is conventionally sent last, so it marks the end of a batch of
answers. See the `query` example for the full pattern, and
[`Screen::capabilities`] for what the facade detects on `init` for you.

## Inline vs. fullscreen

With the alternate screen on (after
[`enter_alt_screen`](Screen::enter_alt_screen)) you own the whole window.
Without it — the default — the surface is *inline*: it occupies the full
width but only as many rows as you draw, anchored in the normal buffer so
scrollback above and the returning shell prompt below stay intact. Both are
diffed cell by cell.

## Features

- `unicode-rs` *(default)* — pure-Rust width and segmentation tables. Small
  and fast.
- `icu` — ICU4X-backed segmentation and properties. Larger build, more
  correct on emoji and grapheme edge cases. Takes precedence when both are
  on.
- `async` — adds [`EventStream`], a runtime-agnostic
  `futures_core::Stream` of events. Pulls in only `futures-core`.

## Install

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

uncurses runs on Linux, macOS, Windows, and the BSDs, and tracks the latest
stable Rust on the 2024 edition.

## Learn by example

The workspace [`examples/`](../examples/README.md) directory is full of
runnable demos grouped by use case — read input only, draw only, the full
mix, mouse, async, inline prompts, and a two-pane file explorer. A few good
first stops:

```sh
cargo run --example input_only   # decode and print events, no rendering
cargo run --example draw_only    # animate the screen, no input
cargo run --example offscreen    # render to a Vec<u8>, no terminal
cargo run --example counter      # the full loop: render + keys + mouse
```

## License

MIT. See [LICENSE](../LICENSE).

[`Screen`]: https://docs.rs/uncurses/latest/uncurses/screen/struct.Screen.html
[`Screen::stdio`]: https://docs.rs/uncurses/latest/uncurses/screen/struct.Screen.html
[`Screen::open`]: https://docs.rs/uncurses/latest/uncurses/screen/struct.Screen.html
[`ScreenOptions`]: https://docs.rs/uncurses/latest/uncurses/screen/struct.ScreenOptions.html
[`Event`]: https://docs.rs/uncurses/latest/uncurses/event/enum.Event.html
[`Key`]: https://docs.rs/uncurses/latest/uncurses/event/struct.Key.html
[`Style`]: https://docs.rs/uncurses/latest/uncurses/style/struct.Style.html
[`Terminal::stdio`]: https://docs.rs/uncurses/latest/uncurses/terminal/struct.Terminal.html
[`Terminal::open`]: https://docs.rs/uncurses/latest/uncurses/terminal/struct.Terminal.html
[`EventStream`]: https://docs.rs/uncurses/latest/uncurses/event/struct.EventStream.html

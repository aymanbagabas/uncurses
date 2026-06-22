# uncurses

A terminal rendering library for Rust that hands you the building blocks and
gets out of the way. No terminfo, no widget tree, no hidden global state, no
framework - just a modern VT100/xterm-style terminal, talked to directly.

> **Full guides and API reference live on the website:
> [aymanbagabas.github.io/uncurses](https://aymanbagabas.github.io/uncurses/)**
> - this README is just the orientation.

## Quick start

A `Screen` session is bracketed by `init()` and `finish()`. It starts
**inline** (drawing in the normal buffer) with the **cursor visible**; the
alternate screen and a hidden cursor are opt-in.

```rust,no_run
use uncurses::buffer::Bounded;
use uncurses::event::{Event, Key};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?; // raw mode + capability detection; inline, cursor visible
    let w = screen.width();
    screen.resize((w, 1)); // inline: one row tall

    screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
    screen.render()?;

    let q: Key = "q".parse().unwrap();
    while !matches!(screen.read_event()?, Event::KeyPress(k) if k == q) {}

    screen.finish() // restore the terminal - always, one call
}
```

## Pick your layer

`Screen` is `Canvas` + `EventSource` + `Terminal` with the lifecycle wired
up. When you outgrow the defaults, drop a layer - nothing is hidden.

| You want to… | Reach for |
| --- | --- |
| Ship an interactive app fast | **`Screen`** - owns the terminal, renderer, and input |
| Render cells to anything that's `Write` (a `Vec`, a socket, a snapshot) | **`Canvas`** - the cell grid + diffing renderer |
| Turn raw bytes into typed events | **`EventSource`** - keys, mouse, paste, focus, resize, queries |
| Enter raw mode, ask the window size | **`Terminal`** |
| Emit a specific escape sequence | the **`ansi`** module |

Drawing is **infallible** - every draw writes into a buffer and returns
nothing. The one place I/O can fail is `flush`, when the buffer hits the
terminal.

## Learn more

- **[Tutorial](https://aymanbagabas.github.io/uncurses/docs/tutorial/)** - build an app from scratch
- **[How terminals work](https://aymanbagabas.github.io/uncurses/docs/terminals/)** - the mental model
- **[API reference](https://aymanbagabas.github.io/uncurses/api/)** - every module and type
- **[Examples](../examples/README.md)** - runnable demos by use case

## Install

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

Runs on Linux, macOS, Windows, and the BSDs; tracks the latest stable Rust
on the 2024 edition.

Features: `unicode-rs` *(default)* width/segmentation, `icu` for ICU4X-backed
correctness, and `async` for a runtime-agnostic `futures_core::Stream` of
events.

## License

MIT. See [LICENSE](../LICENSE).

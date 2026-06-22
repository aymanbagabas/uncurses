# uncurses

A low-level terminal library for Rust that hands you the building blocks and
gets out of the way. No terminfo, no widget tree, no hidden global state, no
framework, just a modern VT100/xterm-style terminal, talked to directly.

## Docs

Guides, concepts, examples, and the full API reference live on the website:

### [uncurses-website.pages.dev](https://uncurses-website.pages.dev/)

## A taste

A `Screen` session is bracketed by `init()` and `finish()`. It starts inline
(drawing in the normal buffer) with the cursor visible; the alternate screen
and a hidden cursor are opt-in.

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

    screen.finish() // restore the terminal, always, one call
}
```

## Install

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

Runs on Linux, macOS, Windows, and the BSDs; tracks the latest stable Rust on
the 2024 edition.

Features: `unicode-rs` *(default)* width/segmentation, `icu` for ICU4X-backed
correctness, and `async` for a runtime-agnostic `futures_core::Stream` of
events.

## License

MIT. See [LICENSE](../LICENSE).

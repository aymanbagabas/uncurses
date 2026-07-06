# uncurses

A terminal toolkit library for Rust. It hands you the building blocks for a
terminal UI and gets out of the way: no terminfo, no widget tree, no hidden
global state, no framework. Just a modern VT100/xterm-style terminal, talked to
directly.

## Docs

Guides, concepts, examples, and the full API reference live on the website.

### [uncurses.org](https://uncurses.org)

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
    screen.resize((w, 2)); // inline: one text row plus a trailing blank line

    screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
    screen.render()?;

    let q: Key = "q".parse().unwrap();
    while !matches!(screen.read_event()?, Event::KeyPress(k) if k == q) {}

    screen.finish() // restore the terminal, always, one call
}
```

## Install

Not on crates.io yet, so depend on it straight from git:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

Runs on Linux, macOS, Windows, and the BSDs. Uses the 2024 edition and tracks
the latest stable Rust (currently 1.88 or newer).

Features: `unicode-rs` *(default)* for width and segmentation, `icu` for
ICU4X-backed correctness, and `async` for a runtime-agnostic
`futures_core::Stream` of events (a low-level `EventStream` over an
`EventSource`).

## Credits

- [ncurses](https://invisible-island.net/ncurses/): the original the name winks
  at, minus the terminfo baggage.
- [ultraviolet](https://github.com/charmbracelet/ultraviolet): Charm's
  low-level terminal library, the inspiration for the cell and screen model.
- [colorprofile](https://github.com/charmbracelet/colorprofile): Charm's color
  degradation library, the model behind uncurses color profiles.
- [ratatui](https://ratatui.rs): the Rust TUI framework, wired up through
  [`uncurses-ratatui`](../uncurses-ratatui/).

## License

MIT. See [LICENSE](../LICENSE).

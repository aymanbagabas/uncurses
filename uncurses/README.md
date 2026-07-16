# uncurses

A terminal toolkit library for Rust. It hands you the building blocks for a
terminal UI and gets out of the way: no terminfo, no widget tree, no hidden
global state, no framework. Just a modern VT100/xterm-compatible terminal, talked to
directly.

## Usage

The whole lifecycle is three calls: `init()` claims the terminal, `render()`
ships changed cells, `finish()` restores it.

```rust,no_run
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?;
    screen.set_str((0, 0), "hello, uncurses", Style::new());
    screen.render()?;
    screen.finish()
}
```

Add an event loop to react to input. A `Screen` session starts inline (drawing
in the normal buffer) with the cursor visible; the alternate screen and a
hidden cursor are opt-in.

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
    loop {
        let ev = screen.read_event()?;
        screen.observe_event(&ev)?;
        if matches!(ev, Event::KeyPress(k) if k == q) {
            break;
        }
    }

    screen.finish() // restore the terminal, always, one call
}
```

`observe_event` is opt-in. Skip it and reads still work; you only forgo
capability tracking for mouse, kitty keyboard, in-band resize, truecolor, and
grapheme support.

## Install

Not on crates.io yet, so depend on it straight from git:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

Runs on Linux, macOS, Windows, and the BSDs. Uses the 2024 edition and tracks
the latest stable Rust (currently 1.88 or newer).

Features: `unicode-rs` *(default)* for width and segmentation, `icu` for
ICU4X-backed correctness, and `async` for runtime-agnostic `event_stream()`,
an owned `futures_core::Stream` over the screen's own decoder with no executor
dependency.

## Credits

- [ncurses](https://invisible-island.net/ncurses/): the original the name winks
  at, minus the terminfo baggage.
- [ultraviolet](https://github.com/charmbracelet/ultraviolet): Charm's
  terminal library, the inspiration for the cell and screen model.
- [colorprofile](https://github.com/charmbracelet/colorprofile): Charm's color
  degradation library, the model behind uncurses color profiles.
- [ratatui](https://ratatui.rs): the Rust TUI framework, wired up through
  [`uncurses-ratatui`](../uncurses-ratatui/).

## License

MIT. See [LICENSE](../LICENSE).

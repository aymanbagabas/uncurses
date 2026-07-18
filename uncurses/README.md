# uncurses

[![CI](https://github.com/aymanbagabas/uncurses/actions/workflows/ci.yml/badge.svg)](https://github.com/aymanbagabas/uncurses/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)
![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)
[![Website](https://img.shields.io/badge/website-uncurses.org-blue.svg)](https://uncurses.org)

A Rust library for building terminal user interfaces. It provides a direct,
framework-free way to draw to the terminal and read input, giving you control
over every cell and your own event loop, whether you run inline, take over the
full screen, mix the two, or leave the console unmanaged and just shape your
output. It includes a diffing renderer that redraws only what changed,
Unicode-aware width, truecolor styling with automatic downsampling, hyperlinks,
and typed keyboard, mouse, and paste input. Rather than a terminfo database, it
detects capabilities through terminal queries and runs across Linux, macOS, and
Windows.

<p align="center">
  <a href="https://github.com/aymanbagabas/uncurses/blob/main/examples/examples/tour.rs"><img src="https://raw.githubusercontent.com/aymanbagabas/uncurses/main/assets/tour.gif" width="440" alt="tour example"></a>
</p>

## Usage

The whole lifecycle is three calls: `init()` claims the terminal, `render()`
ships changed cells, `finish()` restores it.

```rust,no_run
use uncurses::buffer::Bounded;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?;
    // Inline mode draws in the normal buffer: size the frame to what you
    // draw so it doesn't reserve the whole terminal: one text row plus a trailing blank line.
    let w = screen.width();
    screen.resize((w, 2));
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

That's the shape of it; the full API and guides live at
[uncurses.org](https://uncurses.org).

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

- [ncurses](https://invisible-island.net/ncurses/): the original the name nods
  to, minus the terminfo baggage.
- [ultraviolet](https://github.com/charmbracelet/ultraviolet): Charm's
  terminal library, the inspiration for the cell and screen model.
- [colorprofile](https://github.com/charmbracelet/colorprofile): Charm's color
  degradation library, the model behind uncurses color profiles.
- [ratatui](https://ratatui.rs): the Rust TUI framework, wired up through
  [`uncurses-ratatui`](../uncurses-ratatui/).

## License

MIT. See [LICENSE](../LICENSE).

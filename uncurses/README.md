# uncurses

[![CI](https://github.com/aymanbagabas/uncurses/actions/workflows/ci.yml/badge.svg)](https://github.com/aymanbagabas/uncurses/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org)
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
  <img src="https://raw.githubusercontent.com/aymanbagabas/uncurses/main/assets/tour.gif" width="440" alt="tour example">
</p>

## Usage

The whole lifecycle is three calls: `init()` claims the terminal, `render()`
ships changed cells, `finish()` restores it.

```rust,no_run
use uncurses::event::Event;
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?;
    program.enter_alt_screen()?; // take over the full screen
    program.screen_mut().set_str((0, 0), "hello, uncurses", Style::new());
    program.screen_mut().render()?;
    while !matches!(program.read_event()?, Event::KeyPress(_)) {} // wait for a key
    program.finish()
}
```

`Program` owns the terminal and the input source; the `Screen` inside it owns
the drawing. Add an event loop to react to input. A session starts inline
(drawing in the normal buffer) with the cursor visible; the alternate screen
and a hidden cursor are opt-in.

```rust,no_run
use uncurses::buffer::Bounded;
use uncurses::event::{Event, Key};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?; // raw mode, inline, cursor visible
    let w = program.screen().width();
    program.screen_mut().resize((w, 2)); // one text row plus a trailing blank line

    let screen = program.screen_mut();
    screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
    screen.render()?;

    let q: Key = "q".parse().unwrap();
    loop {
        let ev = program.read_event()?;
        if matches!(ev, Event::KeyPress(k) if k == q) {
            break;
        }
    }

    program.finish() // restore the terminal, always, one call
}
```

`read_event` tracks capabilities as the event passes through, so a synchronous
loop has nothing extra to call. Only the async `event_stream()` bypasses the
program, and that is the one path that needs `observe_event`.

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

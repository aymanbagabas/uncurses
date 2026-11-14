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
can ask the terminal what it supports and runs across Linux, macOS, and
Windows.

<p align="center">
  <img src="https://media.githubusercontent.com/media/aymanbagabas/uncurses/main/assets/tour.gif" width="440" alt="Animated uncurses rendering tour">
</p>

## Usage

`Program` owns the session: raw mode, terminal modes, input, and teardown.
`Screen` is the renderer it draws with: a cell grid that ships only the cells
that changed. Reach for the screen through `screen_mut()`.

```rust,no_run
use uncurses::event::Event;
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    let result = (|| {
        program.init()?;
        program.enter_alt_screen()?;
        let screen = program.screen_mut();
        screen.set_str((0, 0), "hello, uncurses", Style::new());
        screen.render()?;
        while !matches!(program.read_event()?, Event::KeyPress(_)) {}
        Ok(())
    })();
    let finish = program.finish();
    result.and(finish)
}
```

`Program` owns the terminal and the input source; the `Screen` inside it owns
the drawing. Add an event loop to react to input. A session starts inline
(drawing in the normal buffer) with the cursor visible; the alternate screen, a
hidden cursor, and capability queries are opt-in.

```rust,no_run
use uncurses::buffer::Bounded;
use uncurses::event::{Event, Key};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    let result = (|| {
        program.init()?;

        let screen = program.screen_mut();
        let w = screen.width();
        screen.resize((w, 2));
        screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
        screen.render()?;

        let q: Key = "q".parse().unwrap();
        loop {
            let ev = program.read_event()?;
            if matches!(ev, Event::KeyPress(k) if k == q) {
                break;
            }
        }
        Ok(())
    })();
    let finish = program.finish();
    result.and(finish)
}
```

Both examples call `finish()` after success or an error. The closure preserves
the session error while the final call attempts terminal restoration.

For capability discovery, see
[Querying the terminal](https://uncurses.org/docs/guides/querying-the-terminal/).

Only need output? A `Screen` stands alone over any `Write`, independent of any
terminal session:

```rust
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

let mut screen = Screen::new(Vec::new(), (20, 1));
screen.set_str((0, 0), "hello, uncurses", Style::new());
screen.render().unwrap();
assert!(!screen.writer().is_empty());
```

That's the shape of it; the full API and guides live at
[uncurses.org](https://uncurses.org).

## Install

Published releases are available on [crates.io](https://crates.io/crates/uncurses).
These examples follow the current repository source. Use its git dependency:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses" }
```

For a published release, use its version and the matching
[API reference on docs.rs](https://docs.rs/uncurses).

Runs on Linux, macOS, Windows, and the BSDs. Uses the 2024 edition and tracks
the latest stable Rust (currently 1.88 or newer).

Features: `unicode-rs` *(default)* for width and segmentation, `icu` for
ICU4X-backed correctness, and `async` for runtime-agnostic `event_stream()`,
an owned `futures_core::Stream` over the program's input decoder with no
executor dependency.

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

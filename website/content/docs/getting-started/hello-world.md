---
title: "Hello, terminal"
weight: 2
---

This is the smallest complete interactive `Screen` program: initialize the terminal, draw one frame, wait for `q`, and restore the terminal.

## Complete program

```rust
use uncurses::buffer::Bounded;
use uncurses::event::{Event, Key};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?;

    let w = screen.width();
    screen.resize((w, 1));

    screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
    screen.present()?;

    let q: Key = "q".parse().unwrap();
    while !matches!(screen.read_event()?, Event::KeyPress(k) if k == q) {}

    screen.finish()
}
```

## Line by line

```rust
use uncurses::buffer::Bounded;
use uncurses::event::{Event, Key};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;
```

`Screen` is the high-level facade. The `Bounded` trait provides `width()`, `Event` and `Key` describe decoded input, `Style` controls cell styling, and `TextSurface` adds `set_str` to drawable surfaces.

```rust
let mut screen = Screen::stdio()?;
```

Build a screen over process `stdin` and `stdout`. Construction is inert: it creates the `Terminal`, `Canvas`, and `EventSource`, but does not change terminal modes yet.

```rust
screen.init()?;
```

Start the session. `init()` enters raw mode, sizes the canvas, enables always-on defaults such as bracketed paste, and stages capability queries.

A freshly initialized screen starts **inline** in the normal buffer with the **cursor visible**. It does not enter the alternate screen or hide the cursor unless you ask.

```rust
let w = screen.width();
screen.resize((w, 1));
```

Inline screens use the terminal width and keep a caller-chosen height. This example uses one row.

```rust
screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
screen.present()?;
```

Drawing is infallible: `set_str` writes cells into the in-memory grid and returns nothing. `present()` renders the diff and flushes bytes to the terminal, so it can fail with `io::Error`.

```rust
let q: Key = "q".parse().unwrap();
while !matches!(screen.read_event()?, Event::KeyPress(k) if k == q) {}
```

Parse the quit key once, then block for typed events until a matching key press arrives. `read_event()` also observes terminal capability replies that were requested during `init()`.

```rust
screen.finish()
```

`finish()` consumes the screen, tears down staged modes, resets the canvas, flushes the teardown bytes, and restores the terminal's previous raw-mode state. Treat `init()` and `finish()` as a bracket around every `Screen` session.

## Fullscreen variant

For a fullscreen app, enter the alternate screen after `init()`.

```rust
use uncurses::event::{Event, Key};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?;
    screen.enter_alt_screen()?;

    screen.set_str((0, 0), "Hello from the alternate screen. Press q to quit.", Style::new());
    screen.present()?;

    let q: Key = "q".parse().unwrap();
    while !matches!(screen.read_event()?, Event::KeyPress(k) if k == q) {}

    screen.finish()
}
```

`enter_alt_screen()` is opt-in and flushes immediately. `finish()` leaves the alternate screen again, so the normal shell buffer returns when the program exits.

## Where to go next

- Learn what `Screen` wraps in [The four layers]({{< relref "the-layers.md" >}}).
- Build a larger app in the [Tutorial]({{< relref "../tutorial.md" >}}).
- See the `screen` module in the [API reference](/api/).

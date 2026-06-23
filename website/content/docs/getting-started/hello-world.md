---
title: "Hello, terminal"
weight: 2
---

Here is the smallest complete uncurses program. It prints a line, waits for you
to press `q`, and hands the terminal back cleanly.

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
    screen.resize((w, 2));

    screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
    screen.render()?;

    let q: Key = "q".parse().unwrap();
    while !matches!(screen.read_event()?, Event::KeyPress(k) if k == q) {}

    screen.finish()
}
```

Run it, see your line, press `q`, and you are back at the shell prompt. The
inline rows are reset, and your earlier scrollback stays intact. That is the
whole contract.

## Line by line

**Open the screen.**

```rust
let mut screen = Screen::stdio()?;
screen.init()?;
```

`Screen::stdio()` wires the screen to standard input and output. `init()` enters
raw mode and starts capability discovery. A session is bracketed by `init()` and
`finish()`; before `init()`, construction does not change terminal modes.

After `init()`, the screen starts *inline*: it draws in the normal buffer, right
where your cursor already is, and it leaves the cursor visible. The alternate
screen and a hidden cursor are opt-in, which we will get to below.

**Claim some space.**

```rust
let w = screen.width();
screen.resize((w, 2));
```

Inline, the screen owns however many rows you ask for. Here we take the full
width and make it two rows tall: one for the text, and one left empty below it.
That trailing blank row means when the program finishes, the shell prompt comes
back on a fresh line of its own instead of butting up against your last line.

**Draw, then show.**

```rust
screen.set_str((0, 0), "Hello! Press q to quit.", Style::new());
screen.render()?;
```

`set_str` paints text at a column and row into an in-memory frame. Nothing
reaches the terminal until `render()`, which diffs the new frame against what is
already on screen and writes only the difference. Painting cannot fail; only
`render()` talks to the terminal, so only `render()` returns a `Result`.

**Wait for input.**

```rust
let q: Key = "q".parse().unwrap();
while !matches!(screen.read_event()?, Event::KeyPress(k) if k == q) {}
```

`read_event()` blocks until something happens: a keypress, a resize, a paste.
Here we loop until that something is the `q` key. Keys parse from strings, so
`"q"`, `"ctrl+c"`, and `"f1"` all just work.

**Put the terminal back.**

```rust
screen.finish()
```

One call. `finish()` drains pending capability-query replies, resets tracked
modes and the managed area, restores the terminal's prior state, and consumes the
screen so you cannot use it by accident afterward. Arrange for `finish()` to run
before your app returns, including from error paths.

## Going fullscreen

The inline program above shares the screen with your shell. To take over the
whole terminal instead, the way an editor or a dashboard does, opt into the
alternate screen and hide the cursor right after `init()`:

```rust
use uncurses::event::{Event, Key};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init()?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    screen.set_str((0, 0), "Fullscreen. Press q to quit.", Style::new());
    screen.render()?;

    let q: Key = "q".parse().unwrap();
    while !matches!(screen.read_event()?, Event::KeyPress(k) if k == q) {}

    screen.finish()
}
```

Two differences from the inline version. First, `enter_alt_screen()` switches to
the terminal's alternate buffer, so your drawing does not scroll into the
shell's history and the original screen comes back untouched on `finish()`.
Second, there is no manual `resize`: right after `init()`, the screen is already
sized to the terminal. `hide_cursor()` just keeps the blinking caret out of your
layout.

Everything else is the same, `finish()` included. It resets tracked modes, puts
the cursor back, and leaves the alternate screen for you.

## Next steps

You have met `Screen`, the front-door entry point. It is not the only one. The
next page maps out [the layers]({{< relref "the-layers.md" >}})
and when to reach for each.

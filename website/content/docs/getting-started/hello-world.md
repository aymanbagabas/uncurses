---
title: "Hello, terminal"
weight: 2
---

Here is the smallest complete uncurses program. It prints a line, waits for you
to press `q`, and hands the terminal back cleanly.

```rust
use uncurses::buffer::Bounded;
use uncurses::event::{Event, Key};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
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

    program.finish()
}
```

Run it, see your line, press `q`, and you are back at the shell prompt. The
inline rows are reset, and your earlier scrollback stays intact. That is the
whole contract.

## Line by line

The snippets below are fragments from the complete program above.

**Open the program.**

```rust
let mut program = Program::stdio()?;
program.init()?;
```

`Program::stdio()` wires a `Program` to standard input and output. The program
owns the terminal session, the input decoder, and a `Screen` renderer. `init()`
enters raw mode and applies the default session options, but it does not query
the terminal. A session is bracketed by `init()` and `finish()`; before `init()`,
construction does not change terminal modes.

After `init()`, the renderer starts *inline*: it draws in the normal buffer,
right where your cursor already is, and it leaves the cursor visible. The
alternate screen and a hidden cursor are opt-in, which we will get to below.

**Claim some space.**

```rust
let screen = program.screen_mut();
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

`screen_mut()` gives you the pure renderer owned by the program. Bind it once
and keep using it, as above; the borrow ends at its last use, so `program` is
free again for the event loop below. `set_str`
paints text at an x/y position into an in-memory frame. Nothing reaches the
terminal until `render()`, which exists on `Screen`, diffs the new frame against
what is already on screen, and writes only the difference. Painting cannot fail;
only `render()` talks to the terminal, so only `render()` returns a `Result`.

**Wait for input.**

```rust
let q: Key = "q".parse().unwrap();
loop {
    let ev = program.read_event()?;
    if matches!(ev, Event::KeyPress(k) if k == q) {
        break;
    }
}
```

`read_event()` blocks until something happens: a keypress, a resize, a paste.
It also observes the event for you, so terminal capability replies and resize
state are recorded as ordinary events pass through the loop. Here we loop until
that event is the `q` key. Keys parse from strings, so `"q"`, `"ctrl+c"`, and
`"f1"` all just work.

**Put the terminal back.**

```rust
program.finish()
```

One call. `finish()` resets the modes this `Program` emitted, resets the managed
area, restores the terminal's prior state, and consumes the program so you
cannot use it by accident afterward. Arrange for `finish()` to run before your
app returns, including from error paths.

If you want terminal capability discovery, it is explicit: call
`program.query_capabilities(&[])?`, then keep reading events until
`Event::PrimaryDeviceAttributes` arrives or your own timeout expires. A normal
`read_event()` loop observes those replies automatically, though sending the
queries and draining them stays yours to do.

## Going fullscreen

The inline program above shares the screen with your shell. To take over the
whole terminal instead, the way an editor or a dashboard does, opt into the
alternate screen and hide the cursor right after `init()`:

```rust
use uncurses::event::{Event, Key};
use uncurses::program::Program;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;

    let screen = program.screen_mut();
    screen.set_str((0, 0), "Fullscreen. Press q to quit.", Style::new());
    screen.render()?;

    let q: Key = "q".parse().unwrap();
    loop {
        let ev = program.read_event()?;
        if matches!(ev, Event::KeyPress(k) if k == q) {
            break;
        }
    }

    program.finish()
}
```

Two differences from the inline version. First, `enter_alt_screen()` switches to
the terminal's alternate buffer and tells the renderer it is now drawing the
whole viewport, so your drawing does not scroll into the shell's history and the
original screen comes back untouched on `finish()`. Second, the renderer is
sized to the whole terminal right after `init()`, so fullscreen needs no
`resize` call. `hide_cursor()` emits the terminal mode and records the matching
render property, keeping the blinking caret out of your layout.

Everything else is the same, `finish()` included. It resets tracked modes, puts
the cursor back, and leaves the alternate screen for you.

## Next steps

You have met `Program`, the front-door entry point for interactive apps. It owns
a pure `Screen` renderer, and the smaller pieces underneath are usable on their
own. The next page maps out [the layers]({{< relref "the-layers.md" >}})
and when to reach for each.

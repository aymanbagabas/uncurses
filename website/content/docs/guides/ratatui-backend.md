---
title: "The ratatui backend"
weight: 10
---

If you already write your UI with [ratatui](https://docs.rs/ratatui) widgets, you
do not have to give that up to get uncurses underneath. The `uncurses-ratatui`
crate is a ratatui `Backend` that renders through a uncurses `Screen`: you keep
writing widgets, and uncurses diffs the frames and ships the minimal bytes.

## Install

Add the backend alongside ratatui:

```toml
[dependencies]
uncurses-ratatui = { git = "https://github.com/aymanbagabas/uncurses" }
ratatui = "0.30"
```

The features mirror the core crate: `unicode-rs` (default), `icu`, and `async`.

## Setup and teardown

`try_init` enters raw mode and the alternate screen and hands you a ready
ratatui `Terminal`; `restore` puts the terminal back. One call each, bracketing
your app.

```rust
use std::io;

fn main() -> io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;
    let result = run(&mut terminal);
    uncurses_ratatui::restore(&mut terminal);
    result
}
```

`try_init` returns an `io::Result`; if you would rather panic on failure, `init`
gives you the terminal directly. There are `*_with_options` variants when you
want to pass `ScreenOptions`, for example to skip the startup capability probe
with `query_capabilities: false`.

## Drawing and input

`terminal.draw` is plain ratatui. Render any widget into the frame; uncurses
turns the resulting buffer into the smallest possible update. Input comes back
from uncurses through the backend.

```rust
use uncurses::event::Event;
use uncurses_ratatui::DefaultTerminal;

fn run(terminal: &mut DefaultTerminal) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| {
            frame.render_widget("Hello from ratatui, via uncurses", frame.area());
        })?;

        let backend = terminal.backend_mut();
        if backend.poll_event(None)? {
            if let Some(Event::KeyPress(_)) = backend.try_read_event() {
                break;
            }
        }
    }
    Ok(())
}
```

`backend_mut` exposes the same `read_event`, `poll_event`, and `try_read_event`
you use on a bare `Screen`, producing uncurses `Event` values. So your render
layer is ratatui and your input layer is uncurses, each doing what it is best
at. Everything from the other guides, mouse, paste, async, querying, works
through the backend's `Screen`.

See the `ratatui_minimal` example for the smallest complete program, and browse
the other `ratatui_*` examples for more, such as popups, inline viewports, and
user input.

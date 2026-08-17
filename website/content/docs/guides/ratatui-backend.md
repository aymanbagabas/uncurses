---
title: "The ratatui backend"
weight: 10
---

If you already write your UI with [ratatui](https://docs.rs/ratatui) widgets, you
do not have to give that up to get uncurses underneath. The `uncurses-ratatui`
crate is a ratatui `Backend` that wraps an uncurses `Program`: ratatui keeps
writing widgets, the program owns input and terminal modes, and its `Screen`
diffs the frames and writes only the necessary bytes.

## Install

Add the backend alongside ratatui:

```toml
[dependencies]
uncurses-ratatui = { git = "https://github.com/aymanbagabas/uncurses" }
ratatui = "0.30"
```

The features mirror the core crate: `unicode-rs` (default), `icu`, and `async`.

## Setup and teardown

`try_init` enters raw mode, switches to the alternate screen, and hands you a
ready ratatui `Terminal`; `restore` restores the terminal state. One call each,
bracketing your app.

```rust
use std::io;

fn main() -> io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;
    let result = run(&mut terminal);
    uncurses_ratatui::restore(&mut terminal);
    result
}

fn run(_: &mut uncurses_ratatui::DefaultTerminal) -> io::Result<()> {
    Ok(())
}
```

`try_init` returns an `io::Result`; if you would rather panic on failure, `init`
gives you the terminal directly. There are `*_with_options` variants when you
want to pass ratatui `TerminalOptions` and uncurses `ProgramOptions`.
`ProgramOptions` is re-exported from `uncurses_ratatui`, defined in
`uncurses::program`, and has exactly four fields: `bracketed_paste`,
`request_pixel_size_on_resize`, `mouse`, and `track_origin`.

```rust
use std::io;

use ratatui::{TerminalOptions, Viewport};
use uncurses_ratatui::ProgramOptions;

fn main() -> io::Result<()> {
    let program_options = ProgramOptions {
        bracketed_paste: false,
        ..ProgramOptions::default()
    };

    let mut terminal = uncurses_ratatui::try_init_with_options(
        TerminalOptions {
            viewport: Viewport::Fullscreen,
        },
        program_options,
    )?;

    uncurses_ratatui::restore(&mut terminal);
    Ok(())
}
```

## Drawing and input

`terminal.draw` is plain ratatui. Render any widget into the frame; uncurses
turns the resulting buffer into the smallest possible update. Input comes from
uncurses through the backend.

```rust
use uncurses::event::Event;
use uncurses_ratatui::DefaultTerminal;

fn run(terminal: &mut DefaultTerminal) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| {
            frame.render_widget("Hello from ratatui, via uncurses", frame.area());
        })?;

        let backend = terminal.backend_mut();
        if !backend.poll_event(None)? {
            continue;
        }
        let Some(ev) = backend.try_read_event()? else {
            continue;
        };
        if let Event::KeyPress(_) = ev {
            break;
        }
    }
    Ok(())
}
```

`backend_mut` exposes the same `read_event`, `poll_event`, and `try_read_event`
you use on a bare `Program`, producing uncurses `Event` values. It also exposes
the wrapped program as `program()` and `program_mut()`, so renderer access is
`backend.program_mut().screen_mut()`. Your render layer is ratatui and your
input layer is uncurses, each doing what it is best at. Features covered in the
other guides, including [mouse input]({{< relref "mouse-input.md" >}}),
[paste]({{< relref "handling-paste.md" >}}), [async events]({{< relref "async-events.md" >}}),
and [terminal queries]({{< relref "querying-the-terminal.md" >}}), work through the backend.

{{< callout type="info" >}}
`init` and `try_init` do not probe the terminal, and no replies are drained.
Querying is opt-in with `backend.program_mut().query_capabilities(&[])?`.
Synchronous reads observe replies automatically. If you read through
`event_stream()`, pass each event to `backend.observe_event(&ev)?`.
{{< /callout >}}

## Async input

With the `async` feature, the backend exposes its own `event_stream()` for a
`tokio::select!` loop. Unlike the sync reads, it yields events without
observing them, so pair it with `observe_event` to keep capability tracking
alive:

```rust
use std::time::Duration;

use tokio_stream::StreamExt;

let mut events = terminal.backend_mut().event_stream();
let mut tick = tokio::time::interval(Duration::from_millis(16));

loop {
    tokio::select! {
        maybe = events.next() => {
            let Some(ev) = maybe else { break };
            let ev = ev?;
            terminal.backend_mut().observe_event(&ev)?;
            // handle input
        }
        _ = tick.tick() => {
            terminal.draw(|frame| {
                frame.render_widget("async ratatui", frame.area());
            })?;
        }
    }
}
```

The stream shares the program's decoder by handle, so you can hold it and still
call mutable backend methods in the same loop. See the [async events guide]({{<
relref "async-events.md" >}}) for the full pattern.

See the `ratatui_minimal` example for the smallest complete program, and browse
the other `ratatui_*` examples for more, such as `ratatui_popup`,
`ratatui_inline`, and `ratatui_user_input`.

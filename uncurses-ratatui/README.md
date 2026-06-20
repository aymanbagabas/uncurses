# uncurses-ratatui

A [`ratatui`](https://docs.rs/ratatui) `Backend` that renders through
[uncurses](../uncurses/README.md). Write your UI with ratatui widgets and
let uncurses diff frames and ship the minimal bytes.

It is built on the high-level [`Screen`](../uncurses/README.md#screen-the-easy-button)
facade: a single [`UncursesBackend`] wraps a `Screen` and drives rendering,
input, and the raw-mode lifecycle through it. You still run your own event
loop — through the same backend.

## Quick start

The `init` / `restore` helpers mirror ratatui's own setup functions: they
enter raw mode, switch to the alternate screen, hide the cursor, and hand
back a ready-to-go ratatui terminal.

```rust,ignore
use ratatui::widgets::Paragraph;
use uncurses::event::Event;

fn main() -> std::io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;

    loop {
        terminal.draw(|frame| {
            frame.render_widget(
                Paragraph::new("from ratatui, via uncurses — press q to quit"),
                frame.area(),
            );
        })?;

        // Read input through the backend (it delegates to the screen).
        let events = terminal.backend_mut();
        if events.poll_event(None)?
            && matches!(events.try_read_event(), Some(Event::KeyPress(_)))
        {
            break;
        }
    }

    uncurses_ratatui::restore(&mut terminal);
    Ok(())
}
```

Always call `terminal.draw(...)` inside your loop, not just once: ratatui
follows terminal resizes on each `draw`, so a one-shot draw never
repaints. See `examples/ratatui_hello.rs` and its `ratatui_*` siblings for
complete programs.

## Reading input

The backend forwards the screen's event API, so you read input straight
off the ratatui terminal:

- [`poll_event`](UncursesBackend::poll_event) — wait up to a timeout for an
  event (`None` blocks).
- [`try_read_event`](UncursesBackend::try_read_event) — take the next
  queued event without blocking.
- [`read_event`](UncursesBackend::read_event) — block for the next event.

These run the screen's capability detection as a side effect, exactly like
the standalone `Screen`.

## Viewports and screen options

Every ratatui viewport works. Pass one — along with your
[`ScreenOptions`] — through `try_init_with_options`:

```rust,ignore
use ratatui::{TerminalOptions, Viewport};
use uncurses_ratatui::ScreenOptions;

let mut terminal = uncurses_ratatui::try_init_with_options(
    TerminalOptions {
        viewport: Viewport::Inline(3),
    },
    ScreenOptions::default(),
)?;
```

- `Fullscreen` and `Fixed` render on the alternate screen.
- `Inline` paints a small region anchored at the cursor on the main
  screen, leaving the surrounding shell output and scrollback intact. See
  `examples/ratatui_inline.rs`.

[`ScreenOptions`] is uncurses' setup knob: bracketed paste, keyboard
enhancements, mouse tracking, in-band resize, and pixel-size behavior. It
is re-exported here so you do not need a separate `uncurses` import just to
tweak it.

## Async input

With the `async` feature, drive the screen's event stream directly for a
runtime-agnostic `futures_core::Stream` of events:

```rust,ignore
use tokio_stream::StreamExt;

while let Some(event) = terminal.backend_mut().screen_mut().events().next().await {
    let event = event?;
    // react to `event`, then terminal.draw(...) here
}
```

Enable it in `Cargo.toml`:

```toml
[dependencies]
uncurses-ratatui = { git = "https://github.com/aymanbagabas/uncurses", features = ["async"] }
```

## Manual setup

If you want full control, build the backend yourself with
[`UncursesBackend::stdio`], [`open`](UncursesBackend::open), or
[`new`](UncursesBackend::new) (which takes a `Screen`), call
[`init`](UncursesBackend::init) / [`init_with`](UncursesBackend::init_with)
and [`restore`](UncursesBackend::restore) yourself, and set the viewport
before handing the backend to `ratatui::Terminal`. The `init` helpers are
exactly this wiring with sensible defaults.

## Install

```toml
[dependencies]
uncurses-ratatui = { git = "https://github.com/aymanbagabas/uncurses" }
ratatui = "0.30"
```

Features mirror the core crate: `unicode-rs` (default), `icu`, and `async`.

## License

MIT. See [LICENSE](../LICENSE).

[`UncursesBackend`]: https://docs.rs/uncurses-ratatui/latest/uncurses_ratatui/struct.UncursesBackend.html
[`UncursesBackend::poll_event`]: https://docs.rs/uncurses-ratatui/latest/uncurses_ratatui/struct.UncursesBackend.html
[`UncursesBackend::try_read_event`]: https://docs.rs/uncurses-ratatui/latest/uncurses_ratatui/struct.UncursesBackend.html
[`UncursesBackend::read_event`]: https://docs.rs/uncurses-ratatui/latest/uncurses_ratatui/struct.UncursesBackend.html
[`UncursesBackend::stdio`]: https://docs.rs/uncurses-ratatui/latest/uncurses_ratatui/struct.UncursesBackend.html
[`UncursesBackend::open`]: https://docs.rs/uncurses-ratatui/latest/uncurses_ratatui/struct.UncursesBackend.html
[`UncursesBackend::new`]: https://docs.rs/uncurses-ratatui/latest/uncurses_ratatui/struct.UncursesBackend.html
[`UncursesBackend::init`]: https://docs.rs/uncurses-ratatui/latest/uncurses_ratatui/struct.UncursesBackend.html
[`UncursesBackend::init_with`]: https://docs.rs/uncurses-ratatui/latest/uncurses_ratatui/struct.UncursesBackend.html
[`UncursesBackend::restore`]: https://docs.rs/uncurses-ratatui/latest/uncurses_ratatui/struct.UncursesBackend.html
[`ScreenOptions`]: https://docs.rs/uncurses/latest/uncurses/screen/struct.ScreenOptions.html

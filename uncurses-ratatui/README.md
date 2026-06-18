# uncurses-ratatui

A [ratatui](https://github.com/ratatui/ratatui) `Backend` built on
[`uncurses::screen::Screen`]. Write your UI with ratatui widgets and let
uncurses sweat the diffing and the bytes on the wire.

Want the bigger picture? The [workspace README](../README.md) has the
project overview, and the [uncurses README](../uncurses/README.md) covers
the core library.

## What it does

`UncursesBackend` owns the whole terminal stack: the `Terminal` handle,
the `Screen`, and a shared `EventSource`. One value drives rendering,
input, and the raw-mode lifecycle. ratatui's draw calls turn into screen
updates, uncurses ships the minimal byte diff, and you still run your own
event loop through the same backend.

## Getting started

The `init`/`restore` helpers mirror ratatui's own setup functions: they
enter raw mode, hide the cursor, pick the screen, and hand back a
ready-to-go ratatui `Terminal`.

```rust,no_run
use std::io;

use ratatui::widgets::{Block, Borders, Paragraph};
use uncurses::event::Event;

fn main() -> io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;
    let result = run(&mut terminal);
    uncurses_ratatui::restore(&mut terminal);
    result
}

fn run(terminal: &mut uncurses_ratatui::DefaultTerminal) -> io::Result<()> {
    loop {
        terminal.draw(|frame| {
            let block = Block::default().title(" hello ").borders(Borders::ALL);
            frame.render_widget(
                Paragraph::new("from ratatui, via uncurses").block(block),
                frame.area(),
            );
        })?;

        // The backend owns the event source; lock it to read input.
        let mut events = terminal.backend().events();
        if events.poll(None)? && matches!(events.try_read(), Some(Event::KeyPress(_))) {
            break;
        }
    }
    Ok(())
}
```

See `examples/ratatui_hello.rs` and its `ratatui_*` siblings for complete
programs, input handling and teardown included.

## Viewports

Every ratatui viewport works. Pass one through `try_init_with_options`:

```rust,ignore
use ratatui::{TerminalOptions, Viewport};

let mut terminal = uncurses_ratatui::try_init_with_options(TerminalOptions {
    viewport: Viewport::Inline(3),
})?;
```

- `Fullscreen` and `Fixed` render on the alternate screen.
- `Inline` paints a small region anchored at the cursor on the main
  screen, leaving the surrounding shell output and scrollback intact. See
  `examples/ratatui_inline.rs`.

## Async input

With the `async` feature, `UncursesBackend::event_stream` hands you a
runtime-agnostic `futures_core::Stream` of events:

```toml
[dependencies]
uncurses-ratatui = { git = "https://github.com/aymanbagabas/uncurses", features = ["async"] }
```

## Manual setup

If you need full control, build the backend directly with
`UncursesBackend::stdio`, `open`, or `new`, call `init`/`restore`
yourself, and set up the screen and viewport before handing it to
`ratatui::Terminal`. The `init` helpers above are exactly this wiring with
sensible defaults.

## License

MIT. See [LICENSE](../LICENSE).

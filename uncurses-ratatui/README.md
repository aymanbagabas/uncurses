# uncurses-ratatui

A [`ratatui`](https://docs.rs/ratatui) `Backend` that renders through
[uncurses](../uncurses/README.md). Write your UI with ratatui widgets; let
uncurses diff frames and ship the minimal bytes. A single `UncursesBackend`
wraps a `Screen` and drives rendering, input, and the raw-mode lifecycle.

> **Full guides and API reference:
> [aymanbagabas.github.io/uncurses](https://aymanbagabas.github.io/uncurses/)**

## Quick start

```rust,ignore
use ratatui::widgets::Paragraph;
use uncurses::event::Event;

fn main() -> std::io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;

    loop {
        terminal.draw(|frame| {
            frame.render_widget(
                Paragraph::new("from ratatui, via uncurses - press q to quit"),
                frame.area(),
            );
        })?; // draw inside the loop so it follows resizes

        let backend = terminal.backend_mut();
        if backend.poll_event(None)? && matches!(backend.try_read_event(), Some(Event::KeyPress(_))) {
            break;
        }
    }

    uncurses_ratatui::restore(&mut terminal);
    Ok(())
}
```

- Read input straight off the backend (`poll_event` / `try_read_event` /
  `read_event`) - it delegates to the screen and runs capability detection.
- Every ratatui viewport works; pass one (and a `ScreenOptions`) through
  `try_init_with_options`. `Inline` paints at the cursor; `Fullscreen` /
  `Fixed` use the alternate screen.
- With the `async` feature, drive `terminal.backend_mut().screen_mut().events()`
  for a `futures_core::Stream` of events.

See `examples/ratatui_*.rs` for complete programs, and the
[API reference](https://aymanbagabas.github.io/uncurses/api/uncurses_ratatui/)
for `UncursesBackend`, viewports, and manual setup.

## Install

```toml
[dependencies]
uncurses-ratatui = { git = "https://github.com/aymanbagabas/uncurses" }
ratatui = "0.30"
```

Features mirror the core crate: `unicode-rs` *(default)*, `icu`, and `async`.

## License

MIT. See [LICENSE](../LICENSE).

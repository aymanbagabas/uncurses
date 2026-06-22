# uncurses-ratatui

A [`ratatui`](https://docs.rs/ratatui) `Backend` that renders through
[uncurses](../uncurses/). Write your UI with ratatui widgets; let uncurses diff
frames and ship the minimal bytes. A single `UncursesBackend` wraps a `Screen`
and drives rendering, input, and the raw-mode lifecycle.

## Docs

Viewports, async input, manual setup, and the full API reference live on the
website:

### [uncurses-website.pages.dev](https://uncurses-website.pages.dev/)

## A taste

```rust,ignore
use ratatui::widgets::Paragraph;
use uncurses::event::Event;

fn main() -> std::io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;

    loop {
        terminal.draw(|frame| {
            frame.render_widget(
                Paragraph::new("from ratatui, via uncurses, press q to quit"),
                frame.area(),
            );
        })?;

        let backend = terminal.backend_mut();
        if backend.poll_event(None)? && matches!(backend.try_read_event(), Some(Event::KeyPress(_))) {
            break;
        }
    }

    uncurses_ratatui::restore(&mut terminal);
    Ok(())
}
```

## Install

```toml
[dependencies]
uncurses-ratatui = { git = "https://github.com/aymanbagabas/uncurses" }
ratatui = "0.30"
```

Features mirror the core crate: `unicode-rs` *(default)*, `icu`, and `async`.

## License

MIT. See [LICENSE](../LICENSE).

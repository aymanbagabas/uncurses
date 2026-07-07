# uncurses-ratatui

A [ratatui](https://ratatui.rs) `Backend` that renders through
[uncurses](../uncurses/). Write your UI with ratatui widgets and let uncurses
diff frames and ship the minimal bytes. A single `UncursesBackend` wraps a
`Screen` and drives rendering, input, and the raw-mode lifecycle. Backend event
reads are pure, like raw `Screen` reads: call `observe_event` to keep
capability tracking alive, or skip it and reads still work.

## Docs

Viewports, input, manual setup, and the full API reference live on the website.

### [uncurses.org](https://uncurses.org)

## A taste

```rust,ignore
use ratatui::widgets::Paragraph;
use uncurses::event::{Event, Key};

fn main() -> std::io::Result<()> {
    let mut terminal = uncurses_ratatui::try_init()?;

    let q: Key = "q".parse().unwrap();
    loop {
        terminal.draw(|frame| {
            frame.render_widget(
                Paragraph::new("from ratatui, via uncurses, press q to quit"),
                frame.area(),
            );
        })?;

        let backend = terminal.backend_mut();
        let ev = backend.read_event()?;
        backend.observe_event(&ev)?;
        if matches!(ev, Event::KeyPress(k) if k == q) {
            break;
        }
    }

    uncurses_ratatui::restore(&mut terminal);
    Ok(())
}
```

## Install

Not on crates.io yet, so depend on it straight from git:

```toml
[dependencies]
uncurses-ratatui = { git = "https://github.com/aymanbagabas/uncurses" }
ratatui = "0.30"
```

Features mirror the core crate: `unicode-rs` *(default)*, `icu`, and runtime-agnostic
`async` for `event_stream()` with no tokio dependency.

## Credits

Built on [ratatui](https://ratatui.rs) and [uncurses](../uncurses/), which in
turn tips its hat to [ncurses](https://invisible-island.net/ncurses/),
[ultraviolet](https://github.com/charmbracelet/ultraviolet), and
[colorprofile](https://github.com/charmbracelet/colorprofile).

## License

MIT. See [LICENSE](../LICENSE).

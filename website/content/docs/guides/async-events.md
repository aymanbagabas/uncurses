---
title: "Async event loops"
weight: 7
---

Most apps can read input synchronously with [`Screen::read_event`](/api/uncurses/screen/struct.Screen.html#method.read_event),
[`Screen::try_read_event`](/api/uncurses/screen/struct.Screen.html#method.try_read_event),
or [`Screen::poll_event`](/api/uncurses/screen/struct.Screen.html#method.poll_event). Those reads are pure now:
they take `&self`, return events, and do not update terminal capabilities.

{{< callout type="info" >}}
Reading from a raw [`Screen`](/api/uncurses/screen/struct.Screen.html) is pure
input. Feeding events back through `screen.observe_event(&ev)?` is optional; it
keeps discovery-driven defaults alive, and skipping it still reads fine.
{{< /callout >}}

With the `async` feature, [`Screen::event_stream`](/api/uncurses/screen/struct.Screen.html#method.event_stream)
gives you a `futures_core::Stream` of `io::Result<Event>` over the screen's own
decoder. The stream is owned, not borrowed, so the same async task can read
input, observe events, resize, and render in one `tokio::select!`. The feature
is runtime-agnostic: it depends on `futures-core`, not tokio.
If you need the shared input source directly, [`Screen::event_source`](/api/uncurses/screen/struct.Screen.html#method.event_source)
returns the `Arc<Mutex<EventSource<I>>>`.

Enable the feature in `Cargo.toml`:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses", features = ["async"] }
```

## The `Screen::event_stream()` path

This is the recommended path for async apps. It mirrors `examples/examples/async_screen.rs`:
terminal input and a frame timer share one `select!`, and the same task renders.

```rust
use std::time::Duration;

use tokio_stream::StreamExt;

use uncurses::buffer::SurfaceMut;
use uncurses::event::{Event, Key};
use uncurses::screen::{Screen, ScreenOptions};
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const FRAME: Duration = Duration::from_millis(16);

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut screen = Screen::stdio()?;
    screen.init_with(ScreenOptions::default())?;
    screen.enter_alt_screen()?;
    screen.hide_cursor()?;

    let result = run(&mut screen).await;
    let finish = screen.finish();
    result.and(finish)
}

async fn run(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let quit_keys: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let mut events = screen.event_stream();
    let mut ticker = tokio::time::interval(FRAME);
    let mut frames = 0u64;

    loop {
        tokio::select! {
            maybe = events.next() => {
                let Some(ev) = maybe else { break };
                let ev = ev?;
                screen.observe_event(&ev)?;

                match ev {
                    Event::KeyPress(ref key) if quit_keys.contains(key) => break,
                    Event::Resize(ws) => screen.resize((ws.col, ws.row)),
                    _ => {}
                }
            }
            _ = ticker.tick() => {
                frames += 1;
                screen.clear();
                screen.set_str((0, 0), &format!("async uncurses frame {frames}"), Style::default());
                screen.render()?;
            }
        }
    }

    Ok(())
}
```

The full game version is `examples/examples/async_arcade.rs`. It uses the same
shape, then adds async game tasks feeding messages into the render loop.

{{< callout type="warning" >}}
`event_stream()` and the sync readers share one source. Use one steady-state
reader. If you run a sync reader and the stream at the same time, each event
goes to whichever consumer drains it first, not both.
{{< /callout >}}

## Observing events is opt-in

The reads above are pure input. `observe_event` is the opt-in call that turns
events into capability state. Skip it and your app still reads input, including
resize events from the OS; you just forgo capability tracking, cached window
sizes, and discovery-driven defaults such as mouse and keyboard upgrades.

## Cleaning up

You do not need to drop `Screen::event_stream()` before
[`Screen::finish`](/api/uncurses/screen/struct.Screen.html#method.finish),
[`Screen::pause`](/api/uncurses/screen/struct.Screen.html#method.pause), or
[`Screen::resume`](/api/uncurses/screen/struct.Screen.html#method.resume). The
stream shares the input source by handle and can stay live across those calls.

## ratatui backend async input

With `uncurses-ratatui`'s `async` feature, the backend has the same pure async
read path. Use [`UncursesBackend::event_stream`](/api/uncurses_ratatui/struct.UncursesBackend.html#method.event_stream)
and pair events you want tracked with
[`UncursesBackend::observe_event`](/api/uncurses_ratatui/struct.UncursesBackend.html#method.observe_event).

```rust
use tokio_stream::StreamExt;
use uncurses::terminal::{Stdin, Stdout};
use uncurses_ratatui::UncursesBackend;

async fn run(backend: &mut UncursesBackend<Stdin, Stdout>) -> std::io::Result<()> {
    let mut events = backend.event_stream();

    while let Some(ev) = events.next().await {
        let ev = ev?;
        backend.observe_event(&ev)?;
        // Handle the event, then draw your ratatui frame.
    }

    Ok(())
}
```

## Low-level `EventSource::into_stream()`

Use this only when you are not using `Screen`. It is the by-hand path: put the
terminal in raw mode yourself, build an [`EventSource`](/api/uncurses/event/struct.EventSource.html)
over the input half, and turn it into an [`EventStream`](/api/uncurses/event/struct.EventStream.html).

```rust
use futures_lite::StreamExt; // or tokio_stream::StreamExt
use uncurses::event::{Event, EventSource, Key};
use uncurses::terminal::Terminal;

async fn run() -> std::io::Result<()> {
    let mut term = Terminal::stdio();
    term.make_raw()?;

    let quit: Key = "ctrl+c".parse().unwrap();
    let mut events = EventSource::new(term.input())?.into_stream();

    while let Some(event) = events.next().await {
        match event? {
            Event::KeyPress(ref k) if *k == quit => break,
            _ => {}
        }
        // React and repaint through your own output half.
    }

    term.restore()
}
```

{{< callout type="info" >}}
Wiring the terminal by hand means no [`Screen`](/api/uncurses/screen/struct.Screen.html)
capability tracking and no cell diffing. Those live on `Screen`. See
[`events`]({{< relref "../concepts/events.md" >}}) for the event model.
{{< /callout >}}

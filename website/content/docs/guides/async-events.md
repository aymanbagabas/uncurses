---
title: "Async event loops"
weight: 7
---

Most apps can read input synchronously with [`Program::read_event`](/api/uncurses/program/struct.Program.html#method.read_event),
[`Program::try_read_event`](/api/uncurses/program/struct.Program.html#method.try_read_event),
or [`Program::poll_event`](/api/uncurses/program/struct.Program.html#method.poll_event). `Program`
owns the terminal session, input source, modes, and capability tracking. Draw
through the pure renderer it contains with `program.screen_mut()`.

{{< callout type="info" >}}
`Program` sync readers observe events automatically, so resize tracking and
capability replies update as events pass through. `try_read_event()` returns
`io::Result<Option<Event>>`, so timeout loops usually write
`while let Some(ev) = program.try_read_event()? { ... }`.
{{< /callout >}}

With the `async` feature, [`Program::event_stream`](/api/uncurses/program/struct.Program.html#method.event_stream)
gives you a `futures_core::Stream` of `io::Result<Event>` over the program's
own decoder. The stream is owned, not borrowed, so the same async task can read
input, observe streamed events, resize, and render in one `tokio::select!`. The
feature is runtime-agnostic: it depends on `futures-core`, not tokio.
If you need the shared input source directly, [`Program::event_source`](/api/uncurses/program/struct.Program.html#method.event_source)
returns the `Arc<Mutex<EventSource<I>>>`.

Enable the feature in `Cargo.toml`:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses", features = ["async"] }
```

## The `Program::event_stream()` path

This is the recommended path for async apps. It mirrors `examples/examples/async_screen.rs`:
terminal input and a frame timer share one `select!`, and the same task renders.

```rust
use std::time::Duration;

use tokio_stream::StreamExt;

use uncurses::buffer::{Bounded, SurfaceMut};
use uncurses::event::{Event, Key};
use uncurses::program::{Program, ProgramOptions};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::terminal::{Stdin, Stdout};
use uncurses::text::TextSurface;

const FRAME: Duration = Duration::from_millis(16);

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init_with(ProgramOptions::default())?;
    program.enter_alt_screen()?;
    program.hide_cursor()?;

    let result = run(&mut program).await;
    let finish = program.finish();
    result.and(finish)
}

async fn run(program: &mut Program<Stdin, Stdout>) -> std::io::Result<()> {
    let quit_keys: [Key; 3] = ["q", "esc", "ctrl+c"].map(|s| s.parse().unwrap());
    let mut events = program.event_stream();
    let mut ticker = tokio::time::interval(FRAME);
    let mut frames = 0u64;

    loop {
        tokio::select! {
            maybe = events.next() => {
                let Some(ev) = maybe else { break };
                let ev = ev?;
                program.observe_event(&ev)?;

                match ev {
                    Event::KeyPress(ref key) if quit_keys.contains(key) => break,
                    Event::Resize(_) => program.autoresize()?,
                    _ => {}
                }
            }
            _ = ticker.tick() => {
                frames += 1;
                draw(program.screen_mut(), frames)?;
            }
        }
    }

    Ok(())
}

fn draw(screen: &mut Screen<Stdout>, frames: u64) -> std::io::Result<()> {
    screen.clear();
    let w = screen.width();
    screen.set_str(
        (0, 0),
        &format!("async uncurses frame {frames}, width {w}"),
        Style::default(),
    );
    screen.render()
}
```

The full game version is `examples/examples/async_arcade.rs`. It uses the same
shape, then adds async game tasks feeding messages into the render loop.

{{< callout type="warning" >}}
`event_stream()` and the sync readers share one source. Use one steady-state
reader. If you run a sync reader and the stream at the same time, each event
goes to whichever consumer drains it first, not both.
{{< /callout >}}

## Observing streamed events

`program.read_event()` and `program.try_read_event()?` observe events for you.
`program.event_stream()` reads from the shared decoder directly, so pair each
streamed event with `program.observe_event(&ev)?` before you act on capability
state or window geometry. That is the same shape used by `async_screen.rs` and
`async_arcade.rs`.

## Cleaning up

You do not need to drop `Program::event_stream()` before
[`Program::finish`](/api/uncurses/program/struct.Program.html#method.finish),
[`Program::pause`](/api/uncurses/program/struct.Program.html#method.pause), or
[`Program::resume`](/api/uncurses/program/struct.Program.html#method.resume). The
stream shares the input source by handle and can stay live across those calls.

## ratatui backend async input

With `uncurses-ratatui`'s `async` feature, the backend has the same shared async
read path. Use [`UncursesBackend::event_stream`](/api/uncurses_ratatui/struct.UncursesBackend.html#method.event_stream)
and pair streamed events with
[`UncursesBackend::observe_event`](/api/uncurses_ratatui/struct.UncursesBackend.html#method.observe_event)
inside the same `select!` loop that draws your ratatui frames.

## Low-level `EventSource::into_stream()`

Use this only when you are not using `Program`. It is the by-hand path: put the
terminal in raw mode yourself, build an [`EventSource`](/api/uncurses/event/struct.EventSource.html)
over the input half, and turn it into an [`EventStream`](/api/uncurses/event/struct.EventStream.html).

```rust
use tokio_stream::StreamExt;
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
Wiring the terminal by hand means no [`Program`](/api/uncurses/program/struct.Program.html)
session management and no cell diffing. Session management lives on `Program`;
cell diffing lives on its [`Screen`](/api/uncurses/screen/struct.Screen.html).
See [`events`]({{< relref "../concepts/events.md" >}}) for the event model.
{{< /callout >}}

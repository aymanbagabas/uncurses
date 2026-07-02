---
title: "Async event loops"
weight: 7
---

Most apps read input synchronously with [`Screen::read_event`]({{< relref
"../concepts/events.md" >}}), which blocks until the next event and runs terminal
capability detection for you. That is the recommended path today.

{{< callout type="warning" >}}
`Screen` does not currently expose an async API. An earlier `Screen::events()` /
`event_reader()` surface was removed while the async story is redesigned. The
low-level `EventStream` below is still available, but it does not integrate with
`Screen` rendering or capability detection. Expect this area to change.
{{< /callout >}}

## The low-level `EventStream`

With the `async` feature, the blocking `EventSource` can be turned into a
`futures_core::Stream` of `io::Result<Event>` with `EventSource::into_stream()`.
A helper thread waits for input readiness and wakes the polling task, so no
blocking call ever lands on your async runtime.

Enable the feature in `Cargo.toml`:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses", features = ["async"] }
```

The feature is runtime-agnostic: it pulls in only `futures-core`, so it runs
under tokio, async-std, smol, or your own executor.

## Awaiting events

This is the by-hand path, without the `Screen` facade: put the terminal in raw
mode yourself, build an `EventSource` over its input, and turn it into a stream.

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
        // ...react and repaint through your own output half...
    }

    drop(events); // stop the helper thread before restoring the terminal
    term.restore()
}
```

Each item is an `io::Result<Event>`, so `?` surfaces read errors the same way the
blocking API does. Because it is a plain `Stream`, you can race it against timers
and channels with your runtime's `select!`.

{{< callout type="info" >}}
Wiring the terminal by hand means no [capability detection]({{< relref
"querying-the-terminal.md" >}}) and no cell diffing: those live on `Screen`.
See the `low_level` example for the full manual setup and teardown.
{{< /callout >}}

## Cleaning up

An `EventStream` owns a helper thread. Drop it before you restore the terminal
(or before [pause and resume]({{< relref "pause-and-resume.md" >}}) if you are
sharing the source) so the thread does not compete for input.

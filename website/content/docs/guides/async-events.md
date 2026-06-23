---
title: "Async event loops"
weight: 7
---

`Screen`'s blocking `read_event` is perfect when input is all your loop does.
But if you also have timers, network I/O, or other tasks to interleave, you want
to `.await` the next event instead of blocking a thread on it. With the `async`
feature, uncurses hands you the event stream as a `futures_core::Stream`.

## Enabling the async feature

Add the feature in `Cargo.toml`:

```toml
[dependencies]
uncurses = { git = "https://github.com/aymanbagabas/uncurses", features = ["async"] }
```

The stream is runtime-agnostic: it depends only on `futures-core`, so it runs
under tokio, async-std, smol, or your own executor. It is backed by a reader
thread, so no blocking call ever lands on your async runtime.

## Awaiting events

`Screen::events()` returns the stream. Drive it with `next().await` from your
`StreamExt` of choice, and the body is the same decode-and-react loop you would
write with `read_event`.

```rust
use futures_lite::StreamExt; // or tokio_stream::StreamExt
use uncurses::event::{Event, Key};

async fn run(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: Key = "ctrl+c".parse().unwrap();

    while let Some(event) = screen.events().next().await {
        match event? {
            Event::KeyPress(ref k) if *k == quit => break,
            Event::Resize(ws) => screen.resize((ws.col, ws.row)),
            _ => {}
        }
        // redraw after reacting to the event
        screen.render()?;
    }
    Ok(())
}
```

Each stream item is an `io::Result<Event>`, so `?` surfaces read errors the same
way the blocking API does.

{{< callout type="info" >}}
`read_event` and `events()` are `Screen` methods. One layer down, an
`EventSource` is the same story under different names: it blocks with `read()`
and gives you the async stream through `into_stream()` (an `EventStream`), with
`try_read()` and `poll()` as its non-blocking and timed reads. This guide stays
at the `Screen` level, but reach for the `EventSource` names when you work below
it.
{{< /callout >}}

## Why `events()` is called in the loop header

`screen.events()` borrows the screen, but only for the single `next().await`. On
edition 2024 that temporary is dropped before the loop body runs, which is why
the body is free to call `screen.render()` and `screen.resize(..)` again. Call
`events()` right in the `while let` header rather than binding the stream to a
variable, and the borrow checker stays happy.

## Racing input against timers and channels

This is where async earns its keep. Because the events are a plain `Stream`, you
can wait on the keyboard and on other work at the same time with your runtime's
`select!`. The classic case is an animation or a clock: a spinner that has to
keep moving even while nobody is typing. With the blocking `read_event` the loop
would be parked on input and the spinner would freeze between keystrokes; a
`select!` lets a timer fire on its own.

```rust
use std::time::Duration;
use tokio_stream::StreamExt;
use uncurses::event::{Event, Key};

async fn run(screen: &mut Screen<Stdin, Stdout>) -> std::io::Result<()> {
    let quit: Key = "ctrl+c".parse().unwrap();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut frame = 0usize;

    loop {
        tokio::select! {
            // input arrived: a key, a resize, a paste...
            event = screen.events().next() => match event {
                Some(event) => match event? {
                    Event::KeyPress(ref k) if *k == quit => break,
                    Event::Resize(ws) => screen.resize((ws.col, ws.row)),
                    _ => {}
                },
                None => break, // the stream ended
            },
            // 100 ms passed with no input: advance the spinner
            _ = tick.tick() => frame = frame.wrapping_add(1),
        }
        draw_spinner(screen, frame);
        screen.render()?;
    }
    Ok(())
}
```

Calling `screen.events().next()` inside the `select!` arm keeps the borrow
scoped to that one poll, the same trick as the `while let` header, so the body
is still free to draw through `screen`. Add more arms as your app grows: an
`mpsc` receiver for messages from background tasks, a socket, a shutdown signal.
The terminal becomes one source among several in your loop.

Pair this with [pause and resume]({{< relref "pause-and-resume.md" >}}) to shell
out from an async app: the stream's reader thread is stopped on `pause` and a
fresh one starts on the next `events()` after `resume`.

See the `async_input` example for a complete tokio-driven loop.

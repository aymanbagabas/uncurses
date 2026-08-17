---
title: "Querying the terminal"
weight: 6
---

Terminals can answer questions about themselves: the background color, the pixel
size of a cell, the current cursor position, and what features they support.
uncurses models this as a request you send and a reply that comes back as an
ordinary event.

## The request and reply model

You write a request, the terminal writes an answer, and that answer arrives in
the same event stream as keystrokes. There is no separate "query" channel.

```mermaid
flowchart TB
  req["program.request_background_color()"]
  req --> term["terminal"]
  term --> ev["Event::BackgroundColor(color) in your event loop"]
```

{{< callout type="info" >}}
`Program::init()` and `Program::init_with()` do not query the terminal. Startup
is quiet, and there is no startup drain. Querying is entirely opt-in.
`ProgramOptions` has no `query_capabilities` field.
{{< /callout >}}

## Capability probing

Call `program.query_capabilities(&extra_bytes)?` when you want uncurses to ask
for its standard capability set. It writes the default queries, then your extra
bytes, then Primary Device Attributes last, and flushes. The Primary DA reply is
the sentinel: because its request was sent last, seeing
`Event::PrimaryDeviceAttributes` means every earlier reply in that batch has
already been delivered.

`query_capabilities` only writes. Consuming the replies is your job, so bound the
wait with `poll_event(Some(timeout))`. A silent terminal never answers, including
the sentinel.

```rust
use std::time::{Duration, Instant};

use uncurses::event::Event;
use uncurses::program::Program;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?;
    program.query_capabilities(&[])?;

    let deadline = Instant::now() + Duration::from_millis(300);
    while let Some(timeout) = deadline.checked_duration_since(Instant::now()) {
        if !program.poll_event(Some(timeout))? {
            break;
        }
        if matches!(program.try_read_event()?, Some(Event::PrimaryDeviceAttributes(_))) {
            break;
        }
    }

    let _caps = program.capabilities();
    program.finish()
}
```

Reads on `Program` auto-observe, so the loop above updates `capabilities()` as it
reads replies. An ordinary event loop gets the same benefit for free: if it keeps
calling `read_event`, `try_read_event`, or `poll_event` plus a read, capability
replies are applied as they arrive.

## Asking one question

`Program` also has `request_*` methods for common queries. Each one sends the
request and flushes; the reply shows up later as an `Event` you match on in your
loop.

```rust
use std::time::{Duration, Instant};

use uncurses::event::Event;
use uncurses::program::Program;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?;
    program.request_background_color()?;

    let deadline = Instant::now() + Duration::from_millis(300);
    'wait: while let Some(timeout) = deadline.checked_duration_since(Instant::now()) {
        if !program.poll_event(Some(timeout))? {
            break;
        }
        while let Some(ev) = program.try_read_event()? {
            if let Event::BackgroundColor(color) = ev {
                let _ = color;
                break 'wait;
            }
        }
    }

    program.finish()
}
```

These cover the everyday questions: the foreground, background, cursor, and
palette colors; the cell and window pixel size; the cursor position; the color
scheme (dark or light); mode state; clipboard contents; and feature probes like
kitty keyboard and modify-other-keys. For the complete set, scan the `request_*`
methods on [`Program`](/api/uncurses/program/struct.Program.html) in the API
reference; each one documents the exact `Event` variant used for its reply.

## Extra queries in the capability batch

If you want your own query to share the Primary DA sentinel, pass its bytes as
`extra`. uncurses writes them after the default capability queries and before the
sentinel, so the same drain loop covers both the built-in replies and yours.

```rust
use std::time::{Duration, Instant};

use uncurses::ansi::color::REQUEST_BACKGROUND_COLOR;
use uncurses::event::Event;
use uncurses::program::Program;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?;
    program.query_capabilities(REQUEST_BACKGROUND_COLOR)?;

    let deadline = Instant::now() + Duration::from_millis(300);
    'wait: while let Some(timeout) = deadline.checked_duration_since(Instant::now()) {
        if !program.poll_event(Some(timeout))? {
            break;
        }
        while let Some(ev) = program.try_read_event()? {
            match ev {
                Event::BackgroundColor(color) => {
                    let _ = color;
                }
                Event::PrimaryDeviceAttributes(_) => break 'wait,
                _ => {}
            }
        }
    }

    program.finish()
}
```

## Asking without a Program

Without `Program`, a request is just bytes you write to the terminal, and the
reply comes back through an `EventSource`. The
[`ansi`](/api/uncurses/ansi/index.html) module has named constants for common
requests, but any escape you write works the same way. Send Primary DA last if
you want a terminator, and use a deadline for terminals that do not answer.

The `query` example (`cargo run --example query`) shows this raw-byte pattern.
The `gradient` example shows the usual app pattern: call
`program.query_capabilities(&[])?`, keep reading events in the normal loop, and
let Program's auto-observation update capabilities as replies arrive.

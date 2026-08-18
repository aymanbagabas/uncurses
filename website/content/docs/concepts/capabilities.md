---
title: "Capabilities"
weight: 10
---

A terminal will tell you about itself if you ask. `Capabilities` is where
[`Program`]({{< relref "program.md" >}}) keeps those answers, stored as the
replies themselves rather than as a summary of them.

## Asking

`init()` leaves the terminal alone, so querying is an explicit call:

```rust,no_run
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

`query_capabilities(&[])` writes the default query set and a Primary DA request
last, then flushes. Consuming the replies is your job. Read events until
`Event::PrimaryDeviceAttributes` arrives, because that reply is the sentinel
that every earlier reply has been delivered. A silent terminal may never answer,
so bound the wait with `poll_event(Some(timeout))`.

An ordinary event loop works just as well, because `read_event()` observes
replies automatically as they pass through. For the full workflow, including
sending your own queries alongside the built-in set, see
[Querying the terminal]({{< relref "../guides/querying-the-terminal.md" >}}).

## Replies, not conclusions

A boolean cannot express the difference between "the terminal said no" and "the
terminal never answered". Every accessor that can be unanswered returns an
`Option`, and `None` always means silence.

```rust,no_run
use uncurses::ansi::mode::{Mode, ModeSetting};
use uncurses::program::Program;

fn main() -> std::io::Result<()> {
    let mut program = Program::stdio()?;
    program.init()?;
    program.query_capabilities(&[])?;
    // ... read events until Event::PrimaryDeviceAttributes ...

    let caps = program.capabilities();

    // The reported state, for any mode you asked about.
    match caps.mode(Mode::SYNCHRONIZED_OUTPUT) {
        Some(ModeSetting::PermanentlySet) => {} // on, and cannot be turned off
        Some(ModeSetting::NotRecognized) => {}  // a definite no
        Some(_) => {}                           // available
        None => {}                              // never answered
    }

    // Or just the yes/no question.
    let _pixels = caps.supports(Mode::MOUSE_SGR_PIXEL);

    // Raw replies for the non-mode queries.
    let _da = caps.primary_device_attributes();
    let _kitty = caps.kitty_keyboard();
    let _mok = caps.modify_other_keys();
    let _name = caps.terminal_name();
    let _sixel = caps.sixel(); // Primary DA attribute 4

    program.finish()
}
```

Because it holds replies, `Capabilities` answers exactly one kind of question:
what the terminal said. Color depth is the instructive case. Direct color is
established by `COLORTERM` and `TERM` as readily as by an XTGETTCAP reply, so
the answer lives on the render profile, `program.screen().color_profile()`,
which already folds in every source. See [Color]({{< relref "color.md" >}}). The
reply itself is still here as `supports_termcap("RGB")` when you want to know
how the profile was reached.

## What the terminal can tell you

Every reply a terminal can send about itself lands here, not just the ones the
program acts on, so anything you send through `query_capabilities`'s `extra`
bytes is readable afterwards. Besides the modes and the values above:

| Reply | Accessor |
| --- | --- |
| Secondary/Tertiary DA | `secondary_device_attributes()`, `tertiary_device_attributes()` |
| XTGETTCAP capability | `termcap(name)`, `supports_termcap(name)`, `termcap_reports()` |
| DECRQSS setting | `setting(selector)`, `settings()` |
| `OSC 10`/`11`/`12` colors | `foreground_color()`, `background_color()`, `cursor_color()` |
| `OSC 4` palette entries | `palette_color(index)`, `palette()` |
| DEC 2031 color scheme | `color_scheme()` |
| Kitty graphics response | `kitty_graphics()` |

`modes()`, `palette()`, `termcap_reports()`, and `settings()` return the whole
map for each, so you can iterate everything the terminal answered.

The environment is the other half of what a program knows, and it never arrives
as a reply at all. `program.env()` is the snapshot the terminal captured, so
`TERM`, `COLORTERM`, and `TERM_PROGRAM` are readable without reaching for
`std::env`, and they stay the values the session started with.
`program.terminal()` borrows the terminal itself for its size and tty queries.
Both are read-only: the program tracks the modes and raw-mode state it emitted
so it can restore them, and a mutable terminal would let that record drift.

## Asking for a current setting

DECRQSS reports current settings rather than what a terminal can do, so it sits
outside the default query set. Ask with
`uncurses::ansi::status::write_decrqss` through `query_capabilities`'s `extra`
bytes, passing the selector of the control function you want, `"m"` for SGR or
`" q"` for the cursor style. Replies are recorded under that same selector. A
private prefix stays part of it, so a `">4m"` request is recorded under
`">m"`, which is what keeps xterm's `XTQMODKEYS` from overwriting SGR.

## Replies that keep arriving

Two of these track the terminal over time rather than answering once.
`color_scheme()` follows the latest DEC 2031 report while
`enable_color_scheme_updates` is on, so it tracks the user toggling dark and
light mode. `kitty_graphics()` is set by any graphics response, including the
acknowledgements a terminal sends while you transmit images.

The colors are the terminal's own, which is the opposite side of what `Program`
tracks for restore. Calling `set_background_color` leaves
`capabilities().background_color()` reporting what the terminal told you, while
the value you sent is what `Program` remembers to undo.

## Options that wait for discovery

`ProgramOptions::prefer_grapheme_clusters` and `prefer_in_band_resize` both
default to `true` and both act on evidence. Each waits for a mode report proving
the terminal supports DEC mode 2027 or 2048, so they stay dormant until you call
`query_capabilities` and read the replies.

Adopting a mode this way goes through the same path as calling
`enable_grapheme_clusters` or `enable_in_band_resize` yourself, so it is
recorded in the emitted-mode set and `finish()` undoes it. Each is enabled at
most once, so a repeated report does not re-emit.

Set either to `false` to keep detection without adoption: the capability is
still recorded in `capabilities()`, and enabling the mode stays your call.

---
title: "Capabilities and queries"
weight: 8
---

uncurses does not ship a terminfo database. Low-level encoders emit the escape
sequences you ask for; unsupported terminals generally ignore what they do not
understand, and higher layers degrade when a capability is not discovered.

Capability detection is explicit. You can ask the terminal by writing request
bytes, then read the reply back as an ordinary `Event`. `Screen::init()` performs
one startup handshake for the features it can use and exposes what it has learned
through `capabilities()`.

See [Events and input]({{< relref "events-and-input.md" >}}), the
[`query` example]({{< relref "../examples.md" >}}#read-input-only), and the
[screen API](/api/uncurses/screen/).

## Request and reply

A terminal query is normal terminal I/O:

1. enter raw mode so replies can be read as bytes;
2. write request bytes, such as Primary Device Attributes or a window-size query;
3. flush the output;
4. read decoded `Event` values from `EventSource`.

The `query` example sends several requests and writes Primary DA last. Its reply
conventionally terminates the batch, so receiving
`Event::PrimaryDeviceAttributes(...)` means earlier replies have had their chance
to arrive.

```rust
use std::io::Write;
use uncurses::ansi::ctrl::REQUEST_PRIMARY_DA;
use uncurses::event::{Event, EventSource};
use uncurses::terminal::Terminal;

fn main() -> std::io::Result<()> {
    let mut term = Terminal::stdio();
    term.make_raw()?;

    let mut out = term.output();
    let mut events = EventSource::new(term.input())?;

    out.write_all(REQUEST_PRIMARY_DA)?;
    out.flush()?;

    let event = events.read()?;
    if let Event::PrimaryDeviceAttributes(attrs) = event {
        let _ = attrs;
    }

    term.restore()
}
```

Replies are just events in the same stream as keys, mouse input, paste, focus,
resize, and other terminal reports.

## The Screen handshake

`Screen::init()` enters raw mode, applies always-on defaults such as bracketed
paste, and stages startup queries. Those replies arrive later through
`read_event()`, `try_read_event()`, or `events()` with the `async` feature.

As each reply passes through the screen, `Screen` updates its internal
capability state. Render-affecting discoveries are applied immediately:
synchronized output wraps frames, grapheme-cluster support changes text width
measurement, and true-color support upgrades the renderer color profile.

A `DECRQM` mode request comes back as `Event::ModeReport`, whose `setting`
distinguishes all five DECRPM states. A terminal can report a mode as
permanently set or permanently reset, meaning it recognizes the mode but will
not let you toggle it. `Screen` only records a mode as an available capability
when `ModeSetting::is_available()` is true, so a permanently reset mode (which
can never be enabled) is not advertised as usable. When you handle
`ModeReport` yourself, prefer `is_available()` over `is_recognized()` for the
same reason.

Discovery-driven defaults from `ScreenOptions` are applied once, when the
terminating Primary DA reply arrives. This means `capabilities()` is a snapshot
of what has been detected so far, not a blocking probe.

## Capabilities

`Screen::capabilities()` returns this `Copy` struct:

```rust
pub struct Capabilities {
    pub synchronized_output: bool,
    pub grapheme_clusters: bool,
    pub in_band_resize: bool,
    pub mouse_normal: bool,
    pub mouse_button: bool,
    pub mouse_any: bool,
    pub mouse_sgr: bool,
    pub mouse_sgr_pixel: bool,
    pub sixel: bool,
    pub clipboard: bool,
    pub kitty_keyboard: bool,
    pub modify_other_keys: bool,
    pub true_color: bool,
}
```

| Field | Meaning |
| --- | --- |
| `synchronized_output` | DEC private mode 2026; frames can be wrapped in synchronized-update markers. |
| `grapheme_clusters` | DEC private mode 2027; text measurement can use grapheme-cluster mode. |
| `in_band_resize` | DEC private mode 2048; resize reports can arrive in the input stream. |
| `mouse_normal` | DEC private mode 1000; normal mouse button tracking. |
| `mouse_button` | DEC private mode 1002; button-event mouse tracking. |
| `mouse_any` | DEC private mode 1003; any-event mouse tracking. |
| `mouse_sgr` | DEC private mode 1006; SGR mouse encoding. |
| `mouse_sgr_pixel` | DEC private mode 1016; SGR-pixel mouse encoding. |
| `sixel` | Primary DA attribute `4`. |
| `clipboard` | Primary DA attribute `52`. |
| `kitty_keyboard` | The terminal answered the Kitty keyboard query. |
| `modify_other_keys` | The terminal answered the xterm `modifyOtherKeys` query. |
| `true_color` | An XTGETTCAP `RGB` or `Tc` reply confirmed direct color. |

## Host responsibility

uncurses gives you the low-level escape encoders, the `EventSource` decoder, and
the `Screen` handshake. It does not hide a terminfo database behind rendering
calls, and it does not make a blocking capability probe every time you draw.

If an application depends on a feature, either use `Screen` and inspect
`capabilities()` after replies have flowed through the event loop, or send the
specific query yourself and handle the reply event.

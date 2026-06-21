---
title: "The Screen facade"
weight: 1
---

`Screen<I, O>` is the high-level facade for terminal applications. It owns the
three pieces most programs otherwise wire together by hand:

- a `Terminal` for the raw-mode lifecycle,
- a `Canvas` for cell-grid drawing and diffed rendering,
- an `EventSource` for decoded input events.

It also tracks non-render terminal and input modes so they can be reset for a
shell handoff and restored later: cursor style, mouse tracking, bracketed
paste, focus reporting, in-band resize reports, color-theme update reports,
window title, pointer shape, xterm `modifyOtherKeys`, and default foreground,
background, cursor, and palette colors.

See the [Screen rustdoc](../api/uncurses/screen/struct.Screen.html) for the full
API surface, and [Canvas and rendering]({{< relref "canvas-and-rendering.md" >}})
for the frame pipeline behind its drawing methods.

## Lifecycle

Construction is inert. `Screen::new`, `Screen::stdio`, and `Screen::open` build
the facade, size the canvas, and create the input decoder, but they do not put
the terminal in raw mode or write startup escape sequences.

Begin a session with `init()` or `init_with(options)`. Initialization enters raw
mode, resizes the canvas to the terminal, applies always-on defaults such as
bracketed paste, and stages capability queries whose replies arrive later
through the event path.

```rust
use uncurses::screen::Screen;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::open()?;
    screen.init()?;

    // draw, present, and read events here

    screen.finish()
}
```

There is no `Drop` teardown. Restoring the terminal is explicit:

| Method | Use |
| --- | --- |
| `finish(self)` | Consume the screen, tear down tracked modes, reset the canvas, flush, and restore the terminal state. |
| `pause(&mut self)` | Temporarily hand the terminal back to the shell without consuming the screen, for example before spawning `$EDITOR`. |
| `resume(&mut self)` | Re-enter raw mode after `pause` or `suspend`, refit the canvas, restore tracked modes, and force a repaint. |
| `suspend(&mut self)` | On Unix, `pause`, raise `SIGTSTP`, then return after the process is foregrounded; call `resume()` next. |

Bracket every session with `init()` or `init_with(...)` at the start and
`finish()` at the end.

## Defaults: inline, visible cursor

A newly initialized `Screen` starts in the normal buffer as an inline surface,
with the cursor visible. It does not enter the alternate screen and does not
hide the cursor unless you ask:

```rust
screen.init()?;
screen.enter_alt_screen()?;
screen.hide_cursor()?;
```

Those setters flush immediately because they change terminal state, not just
the next frame.

## Inline and fullscreen

With the alternate screen off, the managed area is inline: full terminal width,
but only as many rows as the application chooses to draw. With the alternate
screen on, the managed area is the whole viewport.

```text
 Inline (default): the surface lives in the normal buffer, only as
 many rows as you draw; scrollback and the shell prompt stay intact.

   $ earlier shell output
   $ ... scrollback ...
   ┌─────────────────────────┐
   │ managed surface         │  <- only the rows you draw, full width
   └─────────────────────────┘
   $ shell prompt resumes

 Fullscreen (after enter_alt_screen): the whole viewport is the
 surface, addressed with absolute moves, and restored on exit.

   ┌─────────────────────────────┐
   │                             │
   │  the whole terminal         │
   │  viewport is the surface    │
   │                             │
   └─────────────────────────────┘
```

Use `resize((width, height))` to set the canvas size. Inline programs usually
keep the terminal width and choose their own height; fullscreen programs resize
to the whole terminal. `autoresize()` queries the terminal and refits the width;
in fullscreen it also uses the terminal height, while inline mode preserves the
current inline height. `insert_above(content)` pushes lines into the scrollback
above an inline surface.

The `screen_toggle` example shows switching between inline and alternate-screen
mode while preserving a clean teardown.

## ScreenOptions

`init()` is `init_with(ScreenOptions::default())`. Use `init_with(options)` when
startup defaults should differ.

| Field | Default | Effect |
| --- | --- | --- |
| `bracketed_paste` | `true` | Enables DEC private mode 2004 at init so pasted text arrives as paste events. |
| `keyboard_enhancements` | `KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES` | Requests Kitty keyboard enhancements when supported, falling back to xterm `modifyOtherKeys` when Kitty is unavailable. |
| `prefer_in_band_resize` | `true` | Enables DEC private mode 2048 after capability discovery when supported, preferring terminal resize reports over the `SIGWINCH` path. |
| `request_pixel_size_on_resize` | `true` on Windows, `false` elsewhere | Requests `WindowPixelSize` after resize events that carry only cell dimensions, unless in-band resize is active. |
| `mouse` | `None` | When set to `Some(MousePreference { motion, pixels })`, enables mouse tracking after capabilities are known and picks the best supported mode and encoding. |

`MousePreference::default()` requests click reporting in cell coordinates:
`motion: false`, `pixels: false`.

## Capability detection

`init` stages queries for terminal capabilities such as synchronized output,
Unicode grapheme clusters, in-band resize, mouse modes and encodings, Kitty
keyboard support, xterm `modifyOtherKeys`, terminal name, and true color. Their
replies arrive asynchronously through the same read path as user input.

As those replies pass through `read_event`, `try_read_event`, or `events()` with
the `async` feature, `Screen` updates `capabilities()`. Render-affecting
capabilities are applied as they are discovered: synchronized output wraps
frames, grapheme-cluster support changes width measurement, and true-color
support upgrades the renderer profile.

Discovery-driven defaults from `ScreenOptions` are applied once, when the
terminating Primary DA reply arrives.

## Reading events

Use the synchronous event methods when driving a normal loop:

| Method | Behavior |
| --- | --- |
| `read_event()` | Blocks until the next decoded `Event`. |
| `poll_event(timeout)` | Drives the input source for up to `timeout` and returns whether an event is available. |
| `try_read_event()` | Takes the next queued event without doing I/O. |
| `unread_event(event)` | Pushes an event back to the front of the queue. |

With the `async` feature, `events()` returns a stream adapter that yields the
same decoded events and runs the same capability-observation side effects as
`read_event`.

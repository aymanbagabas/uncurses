---
title: "Mouse input"
weight: 2
---

uncurses can report mouse activity as ordinary events: clicks, motion, and the
scroll wheel. You opt in, and after that the pointer shows up in the same event
stream as the keyboard, in cell coordinates or, on terminals that support it,
pixels.

## Turning it on

`MouseTracking` is a bitflags set of optional extras layered on basic button
tracking: `MouseTracking::MOTION` adds pointer movement with no button held, and
`MouseTracking::PIXELS` asks for pixel-accurate coordinates. An empty set
(`MouseTracking::empty()`) is basic tracking with no extras.

To start tracking at init, set `ScreenOptions::mouse` and pass it to `init_with`:

```rust
use uncurses::screen::{MouseTracking, Screen, ScreenOptions};

let mut screen = Screen::stdio()?;
screen.init_with(ScreenOptions {
    mouse: Some(MouseTracking::MOTION),
    ..ScreenOptions::default()
})?;
```

To turn it on and off during a session, call `enable_mouse` with the flags you
want, and `disable_mouse` to stop:

```rust
screen.enable_mouse(MouseTracking::MOTION)?; // motion on, pixels off

// ... later, to stop tracking:
screen.disable_mouse()?;
```

Either way, the screen does not gate mouse setup on detected capabilities. It
always requests 1000 + 1002 tracking and 1006 SGR encoding; `MOTION` also asks
for 1003, and `PIXELS` also asks for 1016. Terminals ignore unsupported modes,
so a terminal that cannot report pixels keeps reporting cells.

## Reading mouse events

Mouse activity arrives as four event kinds, each carrying a position, button,
and modifiers. A mouse event's `x` and `y` are plain `u16`, so build a
`Position` from them and work in the layout types from there.

```rust
use uncurses::event::{Event, MouseButton};
use uncurses::layout::Position;

let ev = screen.read_event()?;
screen.observe_event(&ev)?;

match ev {
    Event::MouseClick(m) => {
        let at = Position::new(m.x, m.y); // use m.button to inspect the button
    }
    Event::MouseRelease(m) => {
        let at = Position::new(m.x, m.y); // the button came back up
    }
    Event::MouseMove(m) => {
        // Motion while a button is held always arrives; buttonless hover
        // motion only when you asked for `MouseTracking::MOTION`.
        let at = Position::new(m.x, m.y);
    }
    Event::MouseWheel(m) => match m.button {
        MouseButton::WheelUp => {}
        MouseButton::WheelDown => {}
        _ => {}
    },
    _ => {}
}
```

Positions are 0-based: `(0, 0)` is the top-left cell, `x` is the column and `y`
is the row.

{{< callout type="info" >}}
Raw `Screen` reads are pure. After `read_event` or `try_read_event` gives you an
event, passing it to `screen.observe_event(&ev)?` is optional; it keeps
capability detection, resize tracking, and discovery defaults alive, and skipping
it still reads fine. `query_capabilities` is the `ScreenOptions` field that
controls whether init sends those probes. The ratatui backend follows the same
pure-read contract.
{{< /callout >}}

## Hit testing

There is no widget tree, so "did they click the button" is a `Rect` containment
check. Track the `Rect` you drew each clickable thing into, then test the event
position against it.

```rust
use uncurses::event::Event;
use uncurses::layout::Rect;

let button = Rect::new(10, 4, 14, 3); // x, y, width, height

if let Event::MouseClick(m) = event {
    if button.contains((m.x, m.y)) {
        // the click landed on the button
    }
}
```

`Rect::contains` takes anything that converts into a `Position`. A `Position` is
`From<(u16, u16)>`, so the tuple form works directly.

## Pixel mode

When you ask for `MouseTracking::PIXELS`, a capable terminal reports the pointer
in pixel offsets instead of cells, which is what you want for sub-cell precision
like dragging a graphic. Two things change, and the screen helps with both.

First, find out whether you are actually getting pixels. The terminal may not
support the request, in which case you quietly keep getting cells.
After the capability replies have been observed, `screen.capabilities()` tells
you which you got:

```rust
let pixel_mode = screen.capabilities().mouse_sgr_pixel;
```

Second, when `pixel_mode` is true, a mouse event's `x` and `y` are pixels, not
columns and rows. `screen.mouse_pixels_to_cells` converts a pixel `Mouse` back
to cell coordinates for you, using the window and cell size the screen already
tracks. With the default size tracking, there is nothing else to set up; the
conversion works once the window pixel size has been observed.

```rust
use uncurses::event::Event;
use uncurses::layout::Position;

if let Event::MouseClick(m) = event {
    let m = if pixel_mode {
        screen.mouse_pixels_to_cells(m).unwrap_or(m)
    } else {
        m // already in cells
    };
    let at = Position::new(m.x, m.y);
}
```

`mouse_pixels_to_cells` returns `None` until uncurses knows the window pixel
size, so `unwrap_or(m)` keeps you going until then. Once a size has been
observed, your hit testing works in cells whether or not the terminal reports
pixels.

{{< callout type="info" >}}
With custom resize settings, make sure the screen learns the window pixel size.
If your resize path does not provide pixel dimensions and
`request_pixel_size_on_resize` is off, `mouse_pixels_to_cells` returns `None`.
Request it yourself once with `screen.request_window_pixel_size()?`, and the
conversion starts working when the reply arrives.
{{< /callout >}}

See `examples/examples/mouse.rs` for a live readout of motion, buttons, and
wheel ticks.

## Clicks in inline mode

When you render [inline]({{< relref "inline-rendering.md" >}}) rather than on the alternate
screen, the surface starts partway down the terminal, but the terminal still
reports clicks in whole-screen coordinates. To hit test against your surface
you need to know where its top-left cell physically sits.

Enabling the mouse inline turns this on automatically: the screen asks the
terminal for the cursor position, records it as the surface origin, and re-asks
on resize. Read it with `screen.origin()`, or map a whole-screen `Mouse`
straight into surface-local coordinates with `screen.mouse_to_origin`, the
origin analogue of `mouse_pixels_to_cells`:

```rust
use uncurses::event::Event;

if let Event::MouseClick(m) = event {
    let local = screen.mouse_to_origin(m); // relative to the surface's top-left
    // hit test `local.x` / `local.y` against your layout
}
```

On the alternate screen the origin is always `(0, 0)`, so `mouse_to_origin` is a
no-op there and the same hit-testing code works in both modes. Origin tracking
is on by default; opt out with `ScreenOptions { track_origin: false, .. }` or
toggle it at runtime with `screen.set_origin_tracking(false)?`.

See `examples/examples/calculator.rs` for a mouse-driven, inline calculator that
maps clicks onto its keypad this way.

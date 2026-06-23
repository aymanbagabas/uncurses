---
title: "Mouse input"
weight: 2
---

uncurses can report mouse activity as ordinary events: clicks, motion, and the
scroll wheel. You opt in, and from then on the pointer shows up in the same event
stream as the keyboard, in cell coordinates or, on terminals that support it,
pixels.

## Turning it on

A `MousePreference` says how much you want to hear: `motion` also reports pointer
movement with no button held, and `pixels` asks for pixel-accurate coordinates.

To start tracking at init, set `ScreenOptions::mouse` and pass it to `init_with`:

```rust
use uncurses::screen::{MousePreference, Screen, ScreenOptions};

let mut screen = Screen::stdio()?;
screen.init_with(ScreenOptions {
    mouse: Some(MousePreference { motion: true, pixels: false }),
    ..ScreenOptions::default()
})?;
```

To turn it on and off during a session, call `enable_mouse` with the same two
flags, and `disable_mouse` to stop:

```rust
screen.enable_mouse(true, false)?; // motion on, pixels off

// ... later, to stop tracking:
screen.disable_mouse()?;
```

Either way, the screen handles the terminal differences for you: it requests
what it needs, and a terminal that cannot do pixels just keeps reporting cells.

## Reading mouse events

Mouse activity arrives as four event kinds, each carrying a position and a
button. A mouse event's `x` and `y` are plain `u16`, so build a
[`Position`]({{< relref "../concepts/_index.md" >}}) from them and work in the
layout types from there.

```rust
use uncurses::event::{Event, MouseButton};
use uncurses::layout::Position;

match screen.read_event()? {
    Event::MouseClick(m) => {
        let at = Position::new(m.x, m.y); // m.button is Left / Right / Middle
    }
    Event::MouseRelease(m) => {
        let at = Position::new(m.x, m.y); // the button came back up
    }
    Event::MouseMove(m) => {
        // Motion while a button is held always arrives; buttonless hover
        // motion only when you asked for `motion: true`.
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

## Hit testing

There is no widget tree, so "did they click the button" is a `Rect` containment
check. Track the [`Rect`]({{< relref "../concepts/_index.md" >}}) you drew each
clickable thing into, then test the event position against it.

```rust
use uncurses::layout::{Position, Rect};

let button = Rect::new(10, 4, 14, 3); // x, y, width, height

if let Event::MouseClick(m) = event {
    if button.contains(Position::new(m.x, m.y)) {
        // the click landed on the button
    }
}
```

`Rect::contains` takes anything that converts into a `Position`, and a `Position`
is `From<(u16, u16)>`, so `button.contains((m.x, m.y))` works too.

## Pixel mode

When you ask for `pixels: true`, a capable terminal reports the pointer in pixel
offsets instead of cells, which is what you want for sub-cell precision like
dragging a graphic. Two things change, and the screen helps with both.

First, find out whether you are actually getting pixels. The terminal may not
support the request, in which case you quietly keep getting cells.
`screen.capabilities()` tells you which you got:

```rust
let pixel_mode = screen.capabilities().mouse_sgr_pixel;
```

Second, when `pixel_mode` is true, a mouse event's `x` and `y` are pixels, not
columns and rows. `screen.mouse_pixels_to_cells` converts a pixel `Mouse` back
to cell coordinates for you, using the window and cell size the screen already
tracks. There is nothing to set up; the conversion just works once the first
size has been observed.

```rust
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

`mouse_pixels_to_cells` returns `None` only in the brief window before any size
has been seen, so `unwrap_or(m)` keeps you going until then. After that your hit
testing always works in cells, whether or not the terminal speaks pixels.

{{< callout type="info" >}}
If you have turned off the screen's automatic size tracking, by disabling
in-band resize and leaving `request_pixel_size_on_resize` off, then nothing
populates the window pixel size and `mouse_pixels_to_cells` keeps returning
`None`. Request it yourself once with `screen.request_window_pixel_size()?`, and
the conversion starts working when the reply arrives.
{{< /callout >}}

See the `mouse` example for a live readout of motion, buttons, and wheel ticks.

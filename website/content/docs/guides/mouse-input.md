---
title: "Mouse input"
weight: 2
---

uncurses decodes terminal mouse reports into typed `Event::MouseClick`, `Event::MouseMove`, `Event::MouseWheel`, and `Event::MouseRelease` values. Use mouse input for clickable controls, hover inspectors, drag-like motion, and wheel navigation.

{{< callout type="info" >}}
Run the basic example with `cargo run --example mouse`. For pixel-accurate sub-cell tracking, read `cargo run --example gradient`.
{{< /callout >}}

## Walk through the examples

### 1. Enable mouse tracking at startup

`mouse.rs` enables tracking through `ScreenOptions`. `motion: true` asks for move events as well as clicks; `pixels: false` keeps coordinates in cells.

```rust
use uncurses::screen::{MousePreference, Screen, ScreenOptions};

let mut screen = Screen::stdio()?;
screen.init_with(ScreenOptions {
    mouse: Some(MousePreference {
        motion: true,
        pixels: false,
    }),
    ..ScreenOptions::default()
})?;
screen.enter_alt_screen()?;
screen.hide_cursor()?;
```

If you need to toggle tracking after startup, use the same preference bits directly:

```rust
screen.enable_mouse(true, false)?; // motion events, cell coordinates
```

### 2. Handle clicks, motion, and the wheel

Every mouse event carries a `Mouse` payload: zero-based `x`, `y`, a `MouseButton`, and active keyboard modifiers. Wheel directions are buttons too, including `WheelUp` and `WheelDown`.

```rust
use uncurses::event::{Event, MouseButton};

match screen.read_event()? {
    Event::MouseMove(m) => state.pointer = Some((m.x, m.y)),
    Event::MouseClick(m) => {
        state.pointer = Some((m.x, m.y));
        state.last_button = Some(m.button);
    }
    Event::MouseWheel(m) => {
        state.pointer = Some((m.x, m.y));
        match m.button {
            MouseButton::WheelUp => state.wheel += 1,
            MouseButton::WheelDown => state.wheel -= 1,
            _ => {}
        }
    }
    Event::Resize(ws) => screen.resize((ws.col, ws.row)),
    _ => {}
}
```

### 3. Request pixel-accurate coordinates

`gradient.rs` asks for SGR-pixel mouse reporting and then adapts when the terminal supports it. Capability replies arrive asynchronously after `init_with`, so the example reads `capabilities().mouse_sgr_pixel` live while resolving a pointer event.

```rust
screen.init_with(ScreenOptions {
    mouse: Some(MousePreference {
        motion: true,
        pixels: true,
    }),
    ..ScreenOptions::default()
})?;
screen.enter_alt_screen()?;
screen.hide_cursor()?;
screen.request_window_pixel_size()?;
```

On resize, refresh the cell-to-pixel ratio:

```rust
Event::Resize(ws) => {
    screen.resize((ws.col, ws.row));
    screen.request_window_pixel_size()?;
}
```

### 4. Convert pixels back to cells

When SGR-pixel mode is active, `Mouse.x` and `Mouse.y` are pixel offsets. `gradient.rs` converts them through the screen's cached window size, then uses `window_pixels()` and `window_cells()` to determine which half of the cell was hit.

```rust
fn resolve(screen: &Screen<Stdin, Stdout>, m: Mouse) -> Option<(u16, u16, u16)> {
    if !screen.capabilities().mouse_sgr_pixel {
        return Some((m.x, m.y, m.x.saturating_mul(2)));
    }

    let cell = screen.mouse_pixels_to_cells(m)?;
    let pixels = screen.window_pixels()?;
    let cells = screen.window_cells().unwrap_or_else(|| screen.size());

    let cell_w = (pixels.width / cells.width.max(1)).max(1);
    let within = m.x.saturating_sub(cell.x * cell_w);
    let right = within >= cell_w / 2;
    let sub = cell.x * 2 + u16::from(right);

    Some((cell.x, cell.y, sub))
}
```

## Common pitfalls

{{< callout type="warning" >}}
Do not cache `capabilities().mouse_sgr_pixel` immediately after `init_with`; detection happens through later input events. If pixel mouse is active, raw mouse coordinates are pixels, not cells, until you call `mouse_pixels_to_cells`.
{{< /callout >}}

## See also

- [The Screen facade]({{< relref "../concepts/screen.md" >}}#screenoptions)
- [Examples]({{< relref "../examples.md" >}}#mouse-and-async)
- [API reference](/api/)

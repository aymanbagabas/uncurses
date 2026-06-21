---
title: "Inline rendering"
weight: 1
---

Inline rendering lets a terminal app draw a managed region in the normal buffer instead of taking over the whole screen. Use it for prompts, pickers, progress views, and other UI that should leave scrollback and the next shell prompt intact.

{{< callout type="info" >}}
Run the main example with `cargo run --example inline_input`. For switching modes, also read `cargo run --example screen_toggle`.
{{< /callout >}}

## Inline versus fullscreen

This is the model documented by `screen/mod.rs`:

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

## Walk through the example

### 1. Initialize, but do not enter the alternate screen

`inline_input.rs` starts in the normal buffer. The important part is what is missing: there is no `enter_alt_screen()` call.

```rust
use uncurses::screen::Screen;

let mut screen = Screen::open()?;
screen.init()?;
screen.hide_cursor()?;

let w = screen.width();
screen.resize((w, 1));
```

`resize((w, 1))` gives the inline surface full terminal width and one row of height. The example grows that height as the input buffer grows.

### 2. Grow the inline height with the content

Before drawing, size the surface to the number of rows the prompt needs.

```rust
let w = self.screen.width();
self.screen.resize((w, self.buffer.lines.len() as u16));
redraw(&mut self.screen, &self.buffer);
self.screen.present()?;
```

On resize events, keep the new terminal width and your chosen inline height:

```rust
match ev {
    Event::Resize(ws) => {
        self.screen.resize((ws.col, self.buffer.lines.len() as u16));
    }
    _ => {}
}
```

### 3. Commit output above the live surface

`inline_input.rs` uses `insert_above` when `Ctrl-D` commits the current multiline block. The committed text scrolls into normal scrollback above the still-live prompt.

```rust
let text = self.buffer.as_text();
self.screen.insert_above(&text);
self.buffer.clear();
```

### 4. Toggle fullscreen when you need it

`screen_toggle.rs` shows the same `Screen` switching layouts. Fullscreen enters the alternate screen and `autoresize()` refits to the whole viewport. Returning inline exits the alternate screen and restores a fixed inline height.

```rust
if self.alt {
    self.screen.enter_alt_screen()?;
    self.screen.autoresize()?;
} else {
    self.screen.exit_alt_screen()?;
    let cols = self.screen.width();
    self.screen.resize((cols, INLINE_ROWS));
}
```

## Common pitfalls

{{< callout type="warning" >}}
Do not call `enter_alt_screen()` for inline UI. Inline is the default after `init()`. Also remember that `autoresize()` preserves the current inline height while updating the width; set the height yourself with `resize((width, height))`.
{{< /callout >}}

## See also

- [The Screen facade]({{< relref "../concepts/screen.md" >}})
- [Canvas and rendering]({{< relref "../concepts/canvas-and-rendering.md" >}})
- [Examples]({{< relref "../examples.md" >}}#inline-rendering-no-alternate-screen)
- [API reference](/api/)

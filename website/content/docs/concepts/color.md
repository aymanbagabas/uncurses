---
title: "Color"
weight: 5
---

Color is part of a cell's [style]({{< relref "cells.md" >}}). You set the color
you want at full fidelity, and uncurses adapts it to what the terminal in front
of you can actually display. You never branch on terminal capability; you state
your intent, and the library does the downsampling.

## Three depths

A `Color` has three representations, and you mix them freely:

| Kind | Range | Example |
| --- | --- | --- |
| `Color::Basic` | the 16 named ANSI colors | `BasicColor::Green` |
| `Color::Indexed` | the 256-color xterm palette | `Color::Indexed(208)` |
| `Color::Rgb` | 24-bit true color | `Color::Rgb(255, 105, 180)` |

All three convert to RGB with `to_rgb`, so palette colors work with the true-color
helpers. `Color::hex` parses `rgb`, `rrggbb`, or `rrggbbaa`, with or without a
leading `#` (alpha is ignored, terminals have none), and `Color::hsl` builds a
color from hue, saturation, and lightness.

## Profiles and downsampling

A [`Profile`](/api/uncurses/color/enum.Profile.html) is the color capability of
an output stream. There are five, ordered from least to most capable:

```
Disabled  <  Ascii  <  Ansi  <  Ansi256  <  TrueColor
```

When a frame is rendered, every color is mapped through the active profile to the
best thing that profile can emit. You always specify the color you mean; the
profile decides how it comes out.

```mermaid
flowchart TB
  color["a color you set (Rgb / Indexed / Basic)"]
  color --> tc["TrueColor: the original color"]
  color --> a256["Ansi256: nearest palette index"]
  color --> ansi["Ansi: nearest of the 16 basics"]
  color --> off["Ascii / Disabled: no color"]
```

`Ascii` drops color but keeps attributes like bold and underline; `Disabled`
drops styling entirely. That is the difference between a frame that is monochrome
but still has structure, and one that is plain text.

## Detection is automatic

A `Screen` picks its profile for you when you call `init`. It starts from the
environment, the same way other CLI tools decide whether to emit color, then
probes the terminal once for direct-color support and upgrades to `TrueColor` if
it gets a confirmation (you can suppress that probe with
`query_capabilities: false`). The environment conventions it reads:

- Output that is not a TTY is `Disabled`, unless `TTY_FORCE` makes it follow TTY
  rules or `CLICOLOR_FORCE` forces color.
- On a TTY, `NO_COLOR` clamps to `Ascii`: no color, but decoration may remain.
- `COLORTERM=truecolor`, `24bit`, `yes`, or `true` upgrades to `TrueColor`,
  except inside `screen`.
- `TERM=dumb` is `Disabled`; `*-256color`, `tmux*`, and `screen*` are `Ansi256`;
  `*-direct` and known true-color terminal names are `TrueColor`.
- `CLICOLOR` raises a non-dumb TTY to at least `Ansi`; `CLICOLOR_FORCE` raises any
  output to at least `Ansi`.

Read the result with `screen.color_profile()`, and override it with
`screen.use_color_profile(..)` when you want to force a level regardless of the
environment.

## Choosing a profile yourself

When you are not using a `Screen`, the
[`Encode`](/api/uncurses/text/trait.Encode.html) trait's `*_with` variants take a
profile directly, so the same painted [buffer]({{< relref "buffers.md" >}}) can
produce full-color escapes for the terminal and plain text for a snapshot test:

```rust
use uncurses::buffer::TextBuffer;
use uncurses::color::{BasicColor, Profile};
use uncurses::style::Style;
use uncurses::text::{Encode, TextSurface};

let mut buffer = TextBuffer::new(6, 1);
buffer.set_str((0, 0), "hello", Style::new().fg(BasicColor::Green));

let colored = buffer.display().to_string(); // TrueColor by default
let plain = buffer.display_with(Profile::Disabled).to_string(); // no escapes
```

This is the basis of the [offscreen rendering]({{< relref
"../guides/offscreen-rendering.md" >}}) guide: compose once, emit at whatever
color level the destination needs.

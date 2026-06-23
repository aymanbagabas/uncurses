---
title: "Styling text"
weight: 3
---

Every bit of text you draw carries a `Style`: foreground and background color,
attributes like bold and italic, an underline shape and color, and optionally an
OSC 8 hyperlink. You build a style with chained methods and hand it to whatever
paints the text.

## Building a style

`Style::new()` (or `Style::default()`) starts empty: no color, no attributes, no
underline, and no link. Builder methods return an updated style, so you chain
only what you want.

```rust
use uncurses::color::{BasicColor, Color};
use uncurses::style::{Style, UnderlineStyle};

let heading = Style::new()
    .bold()
    .fg(BasicColor::BrightCyan);

let warning = Style::new()
    .fg(Color::Rgb(255, 105, 180))
    .bg(BasicColor::Black)
    .italic();

let spelling = Style::new()
    .underline()
    .underline_style(UnderlineStyle::Curly)
    .underline_color(BasicColor::BrightRed);
```

Attributes include `bold`, `faint`, `italic`, `underline`, `reverse`,
`strikethrough`, `conceal`, `blink`, and `rapid_blink`. Underline shapes go
beyond a single line: double, curly, dotted, and dashed, and each can have an
underline color.

## Colors

Colors come in three representations, and you can use them freely:

```rust
Style::new().fg(BasicColor::Green);        // 16-color
Style::new().fg(Color::Indexed(208));      // 256-color palette
Style::new().fg(Color::Rgb(255, 105, 180)); // 24-bit truecolor
```

You always specify the color you mean. When the screen renders to a terminal
that cannot do truecolor, the renderer downsamples to the nearest supported
color through its [color profile]({{< relref "../concepts/color.md" >}}); you do
not branch on terminal capability yourself.

## Painting styled text

On a `Screen`, `TextBuffer`, or any other `TextSurface`, pass the style as the
third argument to `set_str`:

```rust
use uncurses::text::TextSurface;

screen.set_str((2, 1), "Heading", heading);
screen.set_str((2, 3), "be careful", warning);
```

`set_str` paints the text literally: the style applies to the whole run, and any
escape sequences in the string are drawn as visible characters, not interpreted.

## Inline escapes with a Painter

Sometimes the string you want to draw already *contains* SGR or OSC 8 escapes,
perhaps from another program, a log line with ANSI codes, or a snippet you
assembled by hand. A [`Painter`]({{< relref "../concepts/surfaces.md" >}})
interprets those inline escapes and turns them into cell styles, where the
default `set_str` would draw them literally.

Wrap any `TextSurface`, such as a `Screen` or `TextBuffer`, in a `Painter` and
paint through it:

```rust
use uncurses::style::Style;
use uncurses::text::{Painter, TextSurface};

Painter::new(&mut screen).set_str(
    (2, 1),
    "plain \x1b[1;32mbold green\x1b[0m back to plain",
    Style::new(),
);
```

The `\x1b[1;32m` and `\x1b[0m` are read as `Painter` walks the string, so
"bold green" lands bold and green while the rest stays plain. The `Style` you
pass is the *base*: inline escapes layer on top of it, winning where they set a
field and letting the base fill in the rest. An inline reset (`\x1b[0m`) clears
the inline state, so the cells after it fall back to your base rather than to
the terminal default. Had you passed `Style::new().fg(BasicColor::Blue)` above,
"back to plain" would return blue, not colorless.

The painter keeps no style of its own between calls: every `set_str` starts
fresh from the base you hand it, so calls never bleed into each other. It owns
no cells and has no side effects when dropped, so a one-shot
`Painter::new(&mut surface).set_str(..)` is perfectly idiomatic.

## Hyperlinks

A `Style` can carry an OSC 8 hyperlink with `Style::link`. Any rendered cell
whose style has a link opens the hyperlink automatically, and the diffing
renderer closes or changes the OSC 8 span as the link changes from cell to cell.

```rust
let docs = Style::new()
    .underline()
    .fg(BasicColor::BrightBlue)
    .link("https://github.com/aymanbagabas/uncurses", "");

screen.set_str((2, 5), "uncurses on GitHub", docs);
```

The second argument is the OSC 8 parameter string, usually left empty. A
nonempty value such as `id=docs` asks terminals that support it to treat
separate runs as parts of the same link. That can help when a link wraps across
lines; the exact presentation is terminal-dependent.

## Styling plain output

Off the grid, a `Style` also implements `Display`: formatting it writes the
opener, which is any SGR sequence plus an OSC 8 start if the style carries a
link. Its `reset()` returns a matching `Display` closer that undoes exactly what
the opener set, so styled `println!` follows an open/close pattern:

```rust
let bold = Style::new().bold();
println!("{bold}important{}", bold.reset());
```

The opener is additive, so an empty style writes nothing. The closer only emits
what it needs: an SGR-only style closes with `CSI m`, while a linked style also
writes the OSC 8 terminator. For a self-contained span, use `styled`, which wraps
the text with both for you:

```rust
println!("{}", Style::new().bold().styled("important"));
```

See the `styles` example in `examples/examples/styles.rs` for a full tour of
attributes, underline shapes, colors, and a clickable hyperlink.

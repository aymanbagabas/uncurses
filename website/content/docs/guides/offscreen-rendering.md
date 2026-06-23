---
title: "Offscreen rendering"
weight: 9
---

Not every frame needs a live terminal. A `TextBuffer` is a width-aware cell grid
with no terminal behind it: you paint a whole frame into memory, then serialize
it to escape bytes you can write anywhere. It is the tool for snapshot tests,
transcript recorders, and shipping frames over a socket.

## Painting into memory

`TextBuffer::new(width, height)` gives you a grid that draws with the same
`set_str`, `set_cell`, and `fill_rect` as a `Screen`, but owns no input or
output and never touches raw mode.

```rust
use uncurses::buffer::{SurfaceMut, TextBuffer};
use uncurses::style::Style;
use uncurses::text::TextSurface;

let mut frame = TextBuffer::new(40, 3);
frame.set_str((1, 1), "rendered with no terminal", Style::new());
```

There is no renderer and no diffing here. Every serialization produces a
complete, standalone frame, not a delta against a previous one.

## Serializing to bytes

The [`Encode`](/api/uncurses/text/trait.Encode.html) trait turns the grid into
escape bytes. `encode` writes into any `Write`; `display` gives you a `Display`
value when a string is more convenient.

```rust
use uncurses::text::Encode;

let mut bytes = Vec::new();
frame.encode(&mut bytes)?;        // into a Vec<u8>

let string = frame.display().to_string(); // or straight to a String
```

Write those bytes to stdout and the frame paints inline, right where the cursor
is, because it carries its own SGR and positioning. Send them over a socket and
the other end renders the same frame.

## Plain text and snapshot tests

To pin output for a golden test, you usually want the text without color
escapes. Serialize through a [color `Profile`]({{< relref "../concepts/color.md"
>}}) with the `*_with` variants: `Profile::Disabled` drops all styling and gives
you plain text, while `Profile::Ascii` keeps attributes but strips color.

```rust
use uncurses::color::Profile;

let plain = frame.display_with(Profile::Disabled).to_string();
assert_eq!(plain.trim_end(), "rendered with no terminal");
```

Because the same `TextBuffer` can emit full-color escapes for display and plain
text for assertions, you compose a frame once and check it however the test
needs.

## Reading cells back

The grid is also queryable. Walk it cell by cell to extract the text directly,
which is handy for transcripts or diffing two frames yourself.

```rust
use uncurses::buffer::{Bounded, Surface};
use uncurses::cell::Cell;
use uncurses::layout::Position;

for y in 0..frame.height() {
    let mut line = String::new();
    for x in 0..frame.width() {
        line.push_str(frame.cell(Position::new(x, y)).map_or(" ", Cell::content));
    }
    println!("{}", line.trim_end());
}
```

Nothing here is `TextBuffer`-specific. `cell`, `width`, and `height` come from
the `Surface` and `Bounded` traits, so the same walk reads back any surface: a
`Buffer`, a `Window` view, or even a live `Screen`. Anywhere you can paint, you
can also query.

See the `offscreen` example, which composes a bordered, colored card entirely in
memory and then replays the exact bytes on your terminal.

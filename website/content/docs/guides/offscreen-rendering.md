---
title: "Offscreen rendering"
weight: 9
---

Not every frame needs a live terminal. A `Screen<W>` is a desired cell grid, a
diff renderer, and a writer. The writer can be any `std::io::Write`: a
`Vec<u8>` for tests, a `File` for transcripts, a socket, or a terminal handle.
`Screen::new(writer, size)` is infallible and touches no terminal state, so you
can render offscreen without a `Program` at all.

## Rendering into bytes

Build a screen around a `Vec<u8>`, draw the frame, and call `render`. The escape
bytes land in the vector because that vector is the writer.

```rust
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::new(Vec::new(), (40, 3));
    screen.set_str((0, 0), "rendered with no terminal", Style::new());
    screen.render()?;

    let bytes = screen.into_writer();
    assert!(!bytes.is_empty());
    Ok(())
}
```

No input is opened, raw mode is not enabled, and no query is sent. The only I/O
is the `render` call writing frame bytes to the writer you supplied.

## Writing somewhere else

The writer does not have to be memory. A `File` works the same way, which is
useful for transcripts or fixtures that you replay later.

```rust
use std::fs::File;

use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let file = File::create("target/uncurses-frame.ansi")?;
    let mut screen = Screen::new(file, (40, 3));
    screen.set_str((0, 0), "saved as escape bytes", Style::new());
    screen.render()
}
```

Use the same pattern for any `Write`. If you later write those bytes to a real
terminal, the frame paints inline at that terminal's current cursor.

## Style-free snapshots

For golden tests, set the screen's color profile before rendering. The setter is
plain and infallible: it only changes how styled cells will be encoded by later
frames. `Profile::Disabled` drops styling escapes, leaving only the frame bytes
needed to place the cells.

```rust
use uncurses::color::{Color, Profile};
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() -> std::io::Result<()> {
    let mut screen = Screen::new(Vec::new(), (40, 3));
    screen.set_color_profile(Profile::Disabled);
    screen.set_str((0, 0), "plain snapshot", Style::new().fg(Color::BrightCyan));
    screen.render()?;

    let bytes = screen.into_writer();
    let text = String::from_utf8(bytes).expect("renderer writes utf-8 escapes");
    assert!(text.contains("plain snapshot"));
    Ok(())
}
```

Because the same drawing API works with `Vec<u8>`, `File`, and terminal output,
you compose a frame once and choose only the writer and color profile for the
place it is going.

## Truncation with separate styles

Use `set_str_truncate(pos, s, s_style, tail, tail_style)` to mark text that
overflows a row. The text uses `s_style`; the indicator uses `tail_style`.
The indicator appears only on rows that overflow.

Use `set_str_rect_truncate(rect, s, s_style, tail, tail_style)` to restrict
the same operation to a rectangle. Both methods paint literal text on a
`TextBuffer` or `Screen`. Use `Painter` to interpret inline escapes in
either string, with each supplied style as its starting style.

The `offscreen` example shows green text with a red truncation indicator.

## Reading cells back

`Surface::cell` returns a borrowed `CellRef`. Use its getters directly.
Copy text into your output while the borrowed value is in scope.

```rust
use uncurses::buffer::{Bounded, Surface};
use uncurses::layout::Position;
use uncurses::screen::Screen;
use uncurses::style::Style;
use uncurses::text::TextSurface;

fn main() {
    let mut screen = Screen::new(Vec::new(), (40, 3));
    screen.set_str((0, 0), "rendered with no terminal", Style::new());

    let mut lines = Vec::new();
    for y in 0..screen.height() {
        let mut line = String::new();
        for x in 0..screen.width() {
            if let Some(cell) = screen.cell(Position::new(x, y)) {
                line.push_str(cell.content());
            }
        }
        lines.push(line.trim_end().to_string());
    }

    assert_eq!(lines[0], "rendered with no terminal");
}
```

Nothing here is `Screen`-specific. `cell`, `width`, and `height` come from the
`Surface` and `Bounded` traits, so the same walk reads back any surface: a
`Buffer`, `Window`, `View`, or live `Screen`. Anywhere you can paint, you can
also query.

See the `offscreen` example, which composes a bordered, colored card entirely in
memory and then replays the exact bytes on your terminal.

## Prepare cells and copy a frame

Use `Buffer` to prepare a frame with its own width policy.
`new_cell` accepts exactly one grapheme with a nonzero measured width.
It returns an owned `Cell`, which you can borrow with `as_ref`.

`blit` copies from the source into the target. `blit_rect` copies a source rectangle.
The destination imports the content and styles into its own storage.
Both methods preserve stored widths, even when the target uses another width policy.

```rust
use uncurses::buffer::{Buffer, Surface};
use uncurses::layout::Rect;
use uncurses::style::Style;
use uncurses::text::{Encode, WidthMode};

fn main() -> Result<(), uncurses::cell::CellError> {
    let mut target = Buffer::new(8, 1);
    {
        let mut source = Buffer::new(4, 1).with_width_mode(WidthMode::Grapheme);
        let cell = source.new_cell("世", Style::new())?;
        source.set_cell((0, 0).into(), cell.as_ref());
        source.blit(&mut target, (1, 0).into());
        source.blit_rect(&mut target, Rect::new(0, 0, 2, 1), (4, 0).into());
    }
    assert_eq!(target.cell((1, 0).into()).unwrap().width(), 2);
    assert_eq!(target.display().to_string(), " 世 世");
    Ok(())
}
```

The target remains valid after the source goes out of scope.
Structural operations, such as copies and resize, retain the stored widths.
Policy setters affect subsequent text output and factory calls.

For imports from another grid library, use `Cell::new(content, width)`.
This constructor retains the supplied symbol and width, including arbitrary
symbols that span several graphemes. Choose the width from the source grid.

## Keep an owned snapshot

Use `to_owned` when a cell must outlive the borrowed source.
The snapshot retains the text, width, complete style, and hyperlink.

```rust
use uncurses::buffer::{Buffer, Surface};
use uncurses::style::Style;

let snapshot = {
    let mut source = Buffer::new(4, 1);
    let cell = source.new_cell("e\u{301}", Style::new().bold()).unwrap();
    source.set_cell((0, 0).into(), &cell);
    source.cell((0, 0).into()).unwrap().to_owned()
};
let mut target = Buffer::new(4, 1);
target.set_cell((0, 0).into(), snapshot.as_ref());
assert_eq!(target.cell((0, 0).into()).unwrap(), snapshot);
```

`Buffer::line` returns an iterator of borrowed cells, one per column.
It includes continuation columns. Use `cell.to_owned()` to collect portable
snapshots when you need an owned row.

`TextBuffer` remains a thin wrapper around `Buffer`. Its policy setters update
that buffer. Its grid operations use the `SurfaceMut` defaults.
`Buffer` also has inherent rotation operations with their own boundary rules.
See the [buffer reference](/api/uncurses/buffer/struct.Buffer.html) before you
select an operation for a span that cuts through a wide cell.

Run the [offscreen example](https://github.com/aymanbagabas/uncurses/blob/main/examples/examples/offscreen.rs)
with `cargo run -p examples --example offscreen`.
It demonstrates factories, copies, snapshots, and per-column style changes.

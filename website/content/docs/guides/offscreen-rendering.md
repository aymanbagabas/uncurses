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

## Reading cells back

The grid is also queryable. Walk it cell by cell to extract text directly, which
is handy for transcripts or diffing two frames yourself.

```rust
use uncurses::buffer::{Bounded, Surface};
use uncurses::cell::Cell;
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
            line.push_str(screen.cell(Position::new(x, y)).map_or(" ", Cell::content));
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

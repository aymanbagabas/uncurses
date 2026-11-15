---
title: "Buffers"
weight: 3
---

A `Buffer` holds a rectangular grid of [cells]({{< relref "cells.md" >}}) in memory.
Positions start at the top-left corner. Columns increase to the right.
Rows increase downward. A new buffer contains blank cells.

## Prepare a frame in memory

Use a buffer to compose text, inspect a frame, or prepare reusable content.
The buffer owns the text width policy. It supports both individual cells and
whole strings through the [surface traits]({{< relref "surfaces.md" >}}).

```rust
use uncurses::buffer::{Buffer, Surface};
use uncurses::style::Style;
use uncurses::text::TextSurface;

let mut frame = Buffer::new(20, 5);
frame.set_str((0, 0), "Hi 世", Style::new());
assert_eq!(frame.cell((3, 0).into()).unwrap().content(), "世");
```

Buffer writes change memory. A [`Screen`]({{< relref "screen.md" >}})
renders a frame to bytes. A [`Program`]({{< relref "program.md" >}})
manages the terminal session around that screen.

## Share content between grids

A buffer owns its cell resources. You can copy content into another grid,
then drop the source. The destination keeps the text, styles, links, and widths.
Borrowed reads let you inspect cells. Owned snapshots let you keep individual values.

`TextBuffer` provides a thin wrapper around `Buffer` with the same width policy.
Both support text and frame serialization.

See the [offscreen guide]({{< relref "../guides/offscreen-rendering.md" >}})
for copies and snapshots. See the [buffer reference](/api/uncurses/buffer/struct.Buffer.html)
for row access and grid operations.

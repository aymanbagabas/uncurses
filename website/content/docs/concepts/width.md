---
title: "Width"
weight: 4
---

A terminal measures text in columns. Most letters occupy one column.
Some characters, such as `世`, occupy two. An accent can combine with a letter
and use the same column.

## Measure visible characters

uncurses separates text into *graphemes*. A grapheme is one visible character,
even when several Unicode code points form it. The width policy decides how
many columns that grapheme occupies.

`WidthMode::Wc` adds the widths of the parts. `WidthMode::Grapheme` measures
the whole grapheme. Choose the policy that matches the terminal.
The ambiguous-width setting controls characters that terminals can display
as either narrow or wide.

## Keep measurement with the grid

Each surface supplies a width policy. `Buffer` owns that policy, and
`TextBuffer` uses its buffer's policy. Text methods and the cell factory use
the same settings.

```rust
use uncurses::buffer::{Buffer, Surface};
use uncurses::style::Style;

let frame = Buffer::new(10, 1);
let cell = frame.new_cell("世", Style::new()).unwrap();
assert_eq!(cell.width(), 2);
```

A cell keeps its measured width when you copy it into another grid.
This keeps a prepared frame consistent. Policy changes apply to later text
and factory calls.

A [`Program`]({{< relref "program.md" >}}) coordinates its screen policy with
the terminal modes. This matters because a wrong width can shift all later
text on a row.

See the [offscreen guide]({{< relref "../guides/offscreen-rendering.md" >}})
for policy setup and explicit imports.
The [width reference](/api/uncurses/text/enum.WidthMode.html) explains each measurement rule.
The [screen reference](/api/uncurses/screen/struct.Screen.html#method.set_grapheme_clusters)
explains terminal mode coordination.

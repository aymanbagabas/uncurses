---
title: "Buffers"
weight: 3
---

One cell is a single slot. A whole terminal screen is a grid of them, rows by
columns. A *buffer* is that grid held in memory: a rectangle of cells you can
fill, read, and reshape without anything reaching the terminal yet.

## A grid of cells

A `Buffer` is `width × height` cells, addressed by `(x, y)` with the origin in
the top-left corner. `x` grows to the right, `y` grows downward. A brand-new
buffer is full of [blank cells]({{< relref "cells.md" >}}), ready to paint
over.

<figure class="term-fig"><div class="term-grid" style="grid-template-columns: auto repeat(6, 2.2rem);"><span class="lbl">col:</span><span class="lbl">1</span><span class="lbl">2</span><span class="lbl">3</span><span class="lbl">4</span><span class="lbl">5</span><span class="lbl">6</span><span class="lbl">row 1 (y=0)</span><span>H</span><span>i</span><span></span><span></span><span></span><span></span><span class="lbl">row 2 (y=1)</span><span></span><span></span><span></span><span></span><span></span><span></span><span class="lbl">row 3 (y=2)</span><span></span><span></span><span></span><span></span><span></span><span></span></div><figcaption>A six-by-three cell buffer with <code>Hi</code> painted at the top-left; every other slot is a blank cell.</figcaption></figure>

This is exactly how a terminal emulator thinks of its own screen: not a bag of
pixels, but a grid of character cells it repaints as text arrives. A buffer is
your private copy of that same idea.

## Off-screen by design

A buffer is just memory. Writing into it changes nothing on the terminal. You
compose a full frame in the grid, and only later hand it to something that
actually draws (the Screen does this for you). That seam is what makes the good
tricks possible: diffing one frame against the last, snapshotting a frame in a
test, or serializing it to bytes for a transcript.

## Wide cells still take two columns

The grid follows the same rules as a single cell. A wide grapheme occupies two
columns in its row: the primary in one column and a continuation in the next.
Writing it lays down both at once, so the row stays honest about how many
columns are spoken for. Why that matters, and how uncurses decides what is wide,
is the [Width]({{< relref "width.md" >}}) page.

## Working with a buffer

Make one, paint cells into it by position, and ask about its shape:

```rust
use uncurses::buffer::Buffer;
use uncurses::cell::Cell;

fn main() {
    let mut frame = Buffer::new(20, 5);   // 20 columns, 5 rows of blank cells
    frame.set((0, 0), &Cell::narrow("H"));
    frame.set((1, 0), &Cell::narrow("i"));
    assert_eq!(frame.width(), 20);
    assert_eq!(frame.height(), 5);
}
```

`Buffer` is the plain grid, but it is not the only one. It implements the shared
[surface]({{< relref "surfaces.md" >}}) traits, so the same fill, clear, and copy
operations work on every grid type, and `TextBuffer` builds on it to paint whole
strings while respecting [width]({{< relref "width.md" >}}).

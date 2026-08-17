---
title: "Width"
weight: 4
---

A terminal lays text out in cells. The make-or-break question for any string is
simple to ask and surprisingly hard to answer: how many cells does it take?
Guess wrong by one, and everything after it shifts and the row smears.

## Not every character is one cell

Terminal text comes in three cell widths. Most characters are *narrow* and take
one cell. A few are *wide* and take two cells, like CJK characters. Some take
*zero*: a combining accent stacks onto the glyph before it rather than claiming
a column of its own.

| row / col | 1 | 2 | 3 | 4 |
| --- | --- | --- | --- | --- |
| row 1 | a | 世 | cont | é |

One row, four columns: narrow `a` (width 1), wide `世` (width 2, with its
continuation cell), and `é` (the letter `e` plus a combining accent, still one
cell).

## Graphemes, not bytes or code points

That last one is the catch. An `é` might be a single code point, or it might be
an `e` followed by a separate combining accent. Either way, a human sees one
character, and it fills one cell. uncurses measures the way a human counts: by
*extended grapheme cluster*, so a cluster built from several code points still
lands in the right number of cells. Counting bytes or code points would overcount
and shove the rest of the row sideways.

## Two ways to measure

How a cluster is measured is a policy, captured by
[`WidthMode`](/api/uncurses/text/enum.WidthMode.html):

- **`Wc`** is wcwidth-style: it measures each cluster by its first code point
  and ignores the rest. It is simple, and it matches how older or plainer
  terminals behave. This is the default.
- **`Grapheme`** measures the whole cluster, accounting for variation
  selectors, regional-indicator flags, and zero-width-joiner emoji sequences.
  The cluster boundaries follow the Unicode text-segmentation rules in
  [UTS-29](https://unicode.org/reports/tr29/). Pair it with terminal
  [Unicode Core](https://contour-terminal.org/vt-extensions/unicode-core/) mode
  (DEC mode 2027), which measures display width per grapheme cluster.

## East Asian ambiguous width

A handful of code points are genuinely *ambiguous*
([UAX #11](https://unicode.org/reports/tr11/)): one cell or two depending on the
terminal and font. The `eaw_wide` flag decides which way to count them. uncurses
does not probe your terminal to find out, because that is the host's call to
make, not the library's. There is no reliable, platform-independent way to know
a terminal's choice in advance; a host that needs certainty can probe at runtime
(print the character, then read the cursor position back) and set the flag from
what it learns. You set the flag; uncurses honors it.

## Why a wrong guess hurts

Every cell declares its width, and the renderer and cursor planner trust that
declaration completely. If a string claims one cell but the terminal paints two,
everything after it is off by a column: the cursor lands in the wrong place, the
next write lands on top of the wrong cell, and the careful diff falls apart.
Measuring right is what keeps the grid honest.

## Where width lives

You rarely call the measurement functions yourself. Any
[surface]({{< relref "surfaces.md" >}}) that paints text carries a width mode
and an `eaw_wide` flag, and its string-painting methods use them, laying down
wide primaries and their continuations for you. [`TextBuffer`](/api/uncurses/buffer/struct.TextBuffer.html)
exposes `set_width_mode` and `set_eaw_wide`; a
[screen]({{< relref "screen.md" >}}) carries the same mode so it measures the
way the terminal does. Keeping the two in step is the
[program]({{< relref "program.md" >}})'s job: `enable_grapheme_clusters` emits
DEC mode 2027 and switches the screen's measurement together. When you do want
the raw measurement:

```rust
use uncurses::text::grapheme_width;

fn main() {
    assert_eq!(grapheme_width("a", false), 1);          // narrow Latin letter
    assert_eq!(grapheme_width("世", false), 2);          // wide CJK character
    assert_eq!(grapheme_width("e\u{0301}", false), 1);   // "é" = e + combining accent
}
```

More often you want to measure the way a specific surface will paint, without
threading its mode and `eaw_wide` flag by hand. Every `TextSurface` measures
with its own policy: `str_width` totals a string, `grapheme_width` sizes one
cluster, and `grapheme_cells` walks a string as `(cluster, width)` pairs.

```rust
use uncurses::text::TextSurface;
use uncurses::buffer::TextBuffer;

fn main() {
    let buf = TextBuffer::new(10, 1);
    assert_eq!(buf.str_width("a世"), 3);      // 1 + 2 cells
    assert_eq!(buf.grapheme_width("世"), 2);
    let cells: Vec<_> = buf.grapheme_cells("a世").collect();
    assert_eq!(cells, vec![("a", 1), ("世", 2)]);
}
```
